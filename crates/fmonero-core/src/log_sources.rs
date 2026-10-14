//! Bounded snapshots for follow/pause UI. Reading logs never starts any service.
use crate::{CancelToken, Context, Error};
use serde::Serialize;
use std::{
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::PathBuf,
    time::Duration,
};
use zeroize::{Zeroize, Zeroizing};
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LogSource {
    Wallet,
    Node,
    WalletService,
    NodeService,
    Application,
}
impl LogSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::Wallet => "Wallet",
            Self::Node => "Local node",
            Self::WalletService => "Wallet service",
            Self::NodeService => "Local node service",
            Self::Application => "Application",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LogCursor {
    pub device: u64,
    pub inode: u64,
    pub bytes: u64,
}
#[derive(Clone, Debug, Serialize)]
pub struct LogSnapshot {
    pub source: LogSource,
    pub lines: Vec<String>,
    pub truncated: bool,
    pub cursor: Option<LogCursor>,
    pub rotated: bool,
    pub unavailable: Option<String>,
}
#[derive(Clone, Debug)]
pub struct LogQuery {
    pub source: LogSource,
    pub lines: usize,
    pub search: Option<String>,
    pub previous: Option<LogCursor>,
}
pub const MAX_LINES: usize = 1000;
pub const MAX_LINE_CHARS: usize = 2048;

pub fn read(ctx: &Context, query: &LogQuery, cancel: &CancelToken) -> Result<LogSnapshot, Error> {
    if cancel.is_cancelled() {
        return Err(Error::Interrupted);
    }
    let count = query.lines.clamp(1, MAX_LINES);
    if query.search.as_ref().is_some_and(|s| s.len() > 1024) {
        return Err(Error::usage("Log search is limited to 1024 bytes."));
    }
    let mut result = LogSnapshot {
        source: query.source,
        lines: vec![],
        truncated: false,
        cursor: None,
        rotated: false,
        unavailable: None,
    };
    let raw = Zeroizing::new(match query.source {
        LogSource::Wallet | LogSource::Node => {
            if query.source == LogSource::Node && ctx.settings.node.mode.is_remote() {
                result.unavailable = Some(
                    "Remote mode has no local node log. Select Wallet or Wallet service.".into(),
                );
                return Ok(result);
            }
            let path: PathBuf = if query.source == LogSource::Wallet {
                ctx.paths.run_dir.join("monero-wallet-rpc.log")
            } else {
                ctx.settings.node.log_file.clone()
            };
            use std::io::{Read, Seek, SeekFrom};
            let mut file = match std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(&path)
            {
                Ok(f) => f,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    result.unavailable = Some("This log has not been created yet.".into());
                    return Ok(result);
                }
                Err(_) => return Err(Error::safety(
                    "The log source could not be opened as a regular file without following links.",
                )),
            };
            let meta = file
                .metadata()
                .map_err(|_| Error::failure("Log metadata is unavailable."))?;
            if !meta.is_file() {
                return Err(Error::safety(
                    "The log source must be a regular file, not a symbolic link or device.",
                ));
            }
            result.cursor = Some(LogCursor {
                device: meta.dev(),
                inode: meta.ino(),
                bytes: meta.len(),
            });
            if let Some(previous) = &query.previous {
                result.rotated = previous.device != meta.dev()
                    || previous.inode != meta.ino()
                    || previous.bytes > meta.len();
            }
            let count = meta.len().min(crate::log::MAX_TAIL_BYTES);
            result.truncated = count < meta.len();
            file.seek(SeekFrom::Start(meta.len() - count))
                .map_err(|_| Error::failure("The log window could not be read."))?;
            let mut bytes = Zeroizing::new(Vec::with_capacity(count as usize));
            file.take(count)
                .read_to_end(&mut bytes)
                .map_err(|_| Error::failure("The log window could not be read."))?;
            let mut clean = Zeroizing::new(strip_bytes(&bytes));
            // Never retain a partial first line from a bounded tail window.
            if result.truncated {
                clean
                    .split_once('\n')
                    .map(|(_, tail)| tail.to_string())
                    .unwrap_or_default()
            } else {
                std::mem::take(&mut *clean)
            }
        }
        LogSource::Application => {
            let error = Zeroizing::new(
                crate::state::load(ctx)?
                    .last_error
                    .map(|e| e.text)
                    .unwrap_or_default(),
            );
            strip_controls(&error)
        }
        LogSource::WalletService | LogSource::NodeService => {
            if query.source == LogSource::NodeService && ctx.settings.node.mode.is_remote() {
                result.unavailable = Some("Remote mode has no local node service log.".into());
                return Ok(result);
            }
            // A test context never observes the owner's journal. File fixtures are the
            // Wallet/Node sources; process tests supply an isolated journal executable.
            if crate::paths::under_test_state_dir() {
                result.unavailable =
                    Some("System journal is unavailable in an isolated test context.".into());
                return Ok(result);
            }
            let unit = if query.source == LogSource::WalletService {
                &ctx.settings.wallet.unit
            } else {
                &ctx.settings.node.unit
            };
            let unit_arg = format!("--unit={unit}");
            let n = count.to_string();
            let mut out = crate::proc::run_with_deadline(
                &[
                    "journalctl",
                    "--user",
                    "--no-pager",
                    "--output=cat",
                    "--lines",
                    &n,
                    &unit_arg,
                ],
                Duration::from_secs(3),
                Some(cancel),
            )?;
            let success = out.success();
            let mut clean = Zeroizing::new(strip_bytes(&out.stdout));
            out.stdout.zeroize();
            out.stderr.zeroize();
            if !success {
                result.unavailable = Some("The service journal is unavailable.".into());
                return Ok(result);
            }
            std::mem::take(&mut *clean)
        }
    });
    result.lines = sanitized_lines(&raw)
        .into_iter()
        .filter(|line| {
            query
                .search
                .as_ref()
                .is_none_or(|needle| line.contains(needle))
        })
        .collect();
    if result.lines.len() > count {
        result.truncated = true;
        result.lines.drain(..result.lines.len() - count);
    }
    Ok(result)
}
/// Remove terminal controls first, then suppress whole potentially secret-bearing
/// lines. This is intentionally lossy; the source file is never modified.
pub fn sanitize(line: &str) -> String {
    redact_line(&Zeroizing::new(strip_controls(line)))
}
fn strip_bytes(bytes: &[u8]) -> String {
    let mut text = String::from_utf8_lossy(bytes);
    let clean = strip_controls(&text);
    if let std::borrow::Cow::Owned(value) = &mut text {
        value.zeroize();
    }
    clean
}
fn strip_controls(line: &str) -> String {
    #[derive(Clone, Copy)]
    enum Escape {
        Plain,
        Start,
        Csi,
        Osc,
        String,
        OscEnd,
        StringEnd,
    }
    let mut state = Escape::Plain;
    let mut clean = String::new();
    // Parse complete source line before truncating. Dropping an incomplete control
    // string is essential: its payload must never become visible on the next poll.
    for ch in line.chars() {
        state = match state {
            Escape::Plain => match ch {
                '\u{1b}' => Escape::Start,
                '\u{9b}' => Escape::Csi,
                '\u{9d}' => Escape::Osc,
                '\u{90}' | '\u{98}' | '\u{9e}' | '\u{9f}' => Escape::String,
                _ => {
                    if !ch.is_control() || ch == '\n' {
                        clean.push(ch);
                    }
                    Escape::Plain
                }
            },
            Escape::Start => match ch {
                '[' => Escape::Csi,
                ']' => Escape::Osc,
                'P' | 'X' | '^' | '_' => Escape::String,
                _ => Escape::Plain,
            },
            Escape::Csi => {
                if ('@'..='~').contains(&ch) {
                    Escape::Plain
                } else {
                    Escape::Csi
                }
            }
            Escape::Osc => match ch {
                '\u{7}' | '\u{9c}' => Escape::Plain,
                '\u{1b}' => Escape::OscEnd,
                _ => Escape::Osc,
            },
            Escape::String => match ch {
                '\u{9c}' => Escape::Plain,
                '\u{1b}' => Escape::StringEnd,
                _ => Escape::String,
            },
            Escape::OscEnd => {
                if matches!(ch, '\\' | '\u{7}') {
                    Escape::Plain
                } else {
                    Escape::Osc
                }
            }
            Escape::StringEnd => {
                if ch == '\\' {
                    Escape::Plain
                } else {
                    Escape::String
                }
            }
        };
    }
    clean
}
const REDACTED: &str = "[sensitive log entry redacted]";

/// A raw RPC body or a secret-bearing header can span multiple physical lines.
/// Once one is seen, suppress its continuation until the next timestamped Monero
/// record. A blank line is not proof that a private payload has ended.
fn sanitized_lines(clean: &str) -> Vec<String> {
    let mut suppressed = false;
    let mut lines = Vec::new();
    for line in clean.lines() {
        if starts_record(line) {
            suppressed = false;
        }
        if sensitive_line(line) {
            if !suppressed {
                lines.push(REDACTED.into());
            }
            suppressed = true;
        } else if !suppressed {
            lines.push(line.chars().take(MAX_LINE_CHARS).collect());
        }
    }
    lines
}

fn starts_record(line: &str) -> bool {
    let b = line.as_bytes();
    if b.len() < 20 {
        return false;
    }
    // Monero's timestamped log: YYYY-MM-DD HH:MM:SS[.fraction] ... .
    b[..19].iter().enumerate().all(|(i, ch)| match i {
        4 | 7 => *ch == b'-',
        10 => *ch == b' ' || *ch == b'T',
        13 | 16 => *ch == b':',
        _ => ch.is_ascii_digit(),
    }) && matches!(b[19], b'.' | b' ' | b'\t' | b'Z')
}

fn sensitive_line(clean: &str) -> bool {
    let lower = Zeroizing::new(clean.to_ascii_lowercase());
    if [
        "password",
        "passwd",
        "seed",
        "mnemonic",
        "private key",
        "secret key",
        "spend key",
        "spendkey",
        "view key",
        "viewkey",
        "authorization:",
        "rpc-login",
        "daemon-login",
        "jsonrpc",
        "\"params\"",
        "\"result\"",
        "\"method\"",
        "rpc request",
        "rpc response",
    ]
    .iter()
    .any(|s| lower.contains(s))
        || clean
            .split(|c: char| !c.is_ascii_hexdigit())
            .any(|s| s.len() >= 64)
    {
        return true;
    }
    let trimmed = clean.trim_start();
    // JSON can be pretty printed and its initial line need contain no field name.
    trimmed.starts_with('{') || trimmed.starts_with("[{")
}

fn redact_line(clean: &str) -> String {
    if sensitive_line(clean) {
        return "[sensitive log line redacted]".into();
    }
    clean.chars().take(MAX_LINE_CHARS).collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn file_snapshots_distinguish_missing_empty_rotation_and_refuse_links() {
        let root = std::env::temp_dir().join(format!(
            "fmonero-log-window-{}-{}",
            std::process::id(),
            crate::rpc::random_hex(8).unwrap()
        ));
        let mut ctx = Context::for_test(&root).unwrap();
        ctx.systemctl = crate::context::Systemctl::Fixture(root.join("unused-units"));
        std::fs::create_dir_all(&ctx.paths.run_dir).unwrap();
        let path = ctx.paths.run_dir.join("monero-wallet-rpc.log");
        let mut query = LogQuery {
            source: LogSource::Wallet,
            lines: 2,
            search: None,
            previous: None,
        };
        let cancel = CancelToken::new();
        let missing = read(&ctx, &query, &cancel).unwrap();
        assert!(missing.unavailable.is_some());
        assert!(missing.cursor.is_none());
        std::fs::write(&path, b"").unwrap();
        let empty = read(&ctx, &query, &cancel).unwrap();
        assert!(empty.unavailable.is_none());
        assert!(empty.cursor.is_some());
        std::fs::write(&path, b"one\ntwo\nthree\n").unwrap();
        let bounded = read(&ctx, &query, &cancel).unwrap();
        assert_eq!(bounded.lines, ["two", "three"]);
        assert!(bounded.truncated);
        query.previous = bounded.cursor;
        // Same inode, shorter file: log truncation is also rotation.
        std::fs::write(&path, b"new\n").unwrap();
        let truncated = read(&ctx, &query, &cancel).unwrap();
        assert!(truncated.rotated);
        query.previous = truncated.cursor;
        std::fs::rename(&path, path.with_extension("old")).unwrap();
        std::fs::write(&path, b"replacement\n").unwrap();
        assert!(read(&ctx, &query, &cancel).unwrap().rotated);
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(path.with_extension("old"), &path).unwrap();
        assert!(read(&ctx, &query, &cancel).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn control_strings_remove_entire_payload_including_incomplete_tails() {
        assert_eq!(
            sanitize("before\x1b]52;c;Q0xJUEJPQVJE\x07after"),
            "beforeafter"
        );
        assert_eq!(
            sanitize("before\x1b]8;;https://private.example/path\x1b\\label\x1b]8;;\x1b\\after"),
            "beforelabelafter"
        );
        assert_eq!(
            sanitize("before\x1bPprivate device data\x1b\\after"),
            "beforeafter"
        );
        for tail in [
            "\x1b]52;c;PRIVATE",
            "\x1bPPRIVATE",
            "\x1b[31",
            "\x1b]8;;https://private.example/",
        ] {
            assert_eq!(sanitize(&format!("before{tail}")), "before");
        }
        assert_eq!(sanitize("\x1b[31mordinary\x1b[0m"), "ordinary");
    }
    #[test]
    fn sequence_payload_never_reenters_the_line_buffer_or_search() {
        let raw = "normal\n\x1b]52;c;FIRST\nSECOND\nTHIRD\x07visible\n\x1bPdevice\npayload";
        let clean = strip_controls(raw);
        assert_eq!(clean, "normal\nvisible\n");
        let rows: Vec<_> = clean
            .lines()
            .map(redact_line)
            .filter(|s| s.contains("SECOND"))
            .collect();
        assert!(rows.is_empty());
    }
    #[test]
    fn multiline_private_entries_and_rpc_bodies_do_not_escape_into_search() {
        // Synthetic text, not a real wallet seed or RPC credential.
        let raw = "2026-10-14 00:00:01.000 INFO ordinary ready\n\
                   2026-10-14 00:00:02.000 ERROR mnemonic follows\n\
                   TEST_CONTINUATION_WORDS\n\n\
                   MORE_PRIVATE_TEXT\n\
                   2026-10-14 00:00:03.000 INFO refreshed to height 42\n\
                   {\n\
                     \"result\": {\"sensitive\": \"PRIVATE_JSON_VALUE\"}\n\
                   }\n\
                   2026-10-14 00:00:04.000 WARNING node disconnected";
        let rows = sanitized_lines(raw);
        assert_eq!(
            rows,
            [
                "2026-10-14 00:00:01.000 INFO ordinary ready",
                REDACTED,
                "2026-10-14 00:00:03.000 INFO refreshed to height 42",
                REDACTED,
                "2026-10-14 00:00:04.000 WARNING node disconnected",
            ]
        );
        for forbidden in ["CONTINUATION", "PRIVATE_TEXT", "PRIVATE_JSON"] {
            assert!(!rows.iter().any(|line| line.contains(forbidden)));
        }
        assert_eq!(sanitized_lines("rpc response:\nPRIVATE\n\nEND"), [REDACTED]);
        assert_eq!(
            sanitized_lines("ordinary startup\nready"),
            ["ordinary startup", "ready"]
        );
    }
    #[test]
    fn log_redaction_removes_controls_credentials_and_key_material() {
        assert_eq!(
            sanitize("password: example"),
            "[sensitive log line redacted]"
        );
        assert_eq!(
            sanitize(&format!("key {}", "a".repeat(64))),
            "[sensitive log line redacted]"
        );
        assert!(!sanitize("\x1b[31mordinary\x1b[0m\x07").contains('\x1b'));
        assert_eq!(sanitize(&"x".repeat(5000)).chars().count(), MAX_LINE_CHARS);
    }
}

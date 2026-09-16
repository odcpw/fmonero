//! A bounded tail of a daemon log file (DESIGN.md §5b, §4.1).
//!
//! Signature-only at the Phase 1a gate (§8). Consumed by `node log` and by the TUI's Log
//! screen, which is why it is a core function rather than a subcommand body.

use std::path::{Path, PathBuf};

use crate::error::Error;

/// The lines a tail produced.
pub struct Tail {
    pub lines: Vec<String>,
    /// True when the read stopped at the 4 MiB bound rather than at the start of the file.
    pub truncated: bool,
    pub path: PathBuf,
}

/// The most this ever reads from the end of a log, however many lines were asked for.
pub const MAX_TAIL_BYTES: u64 = 4 * 1024 * 1024;

/// Bounded read of the last `lines` lines, with an optional substring filter.
///
/// A missing file is `Ok(Tail { lines: vec![], .. })`, **never an error**: `node log` with
/// no log yet is a GREEN row (§4b).
pub fn tail(path: &Path, lines: usize, grep: Option<&str>) -> Result<Tail, Error> {
    use std::io::{Read, Seek, SeekFrom};

    let mut f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Tail {
                lines: Vec::new(),
                truncated: false,
                path: path.to_path_buf(),
            })
        }
        Err(e) => {
            return Err(
                Error::failure(format!("cannot read {}: {e}", path.display()))
                    .with_hint("fmonero settings get node.log_file"),
            )
        }
    };
    let size = f
        .metadata()
        .map_err(|e| Error::failure(format!("cannot stat {}: {e}", path.display())))?
        .len();
    // Bounded read: never more than 4 MiB from the end, however many lines were asked for.
    // A `node log --lines 1000000` on a gigabyte log is a command that must still return.
    let want = size.min(MAX_TAIL_BYTES);
    let truncated_by_bytes = want < size;
    f.seek(SeekFrom::Start(size - want))
        .map_err(|e| Error::failure(format!("cannot seek {}: {e}", path.display())))?;
    let mut buf = Vec::with_capacity(want as usize);
    f.take(want)
        .read_to_end(&mut buf)
        .map_err(|e| Error::failure(format!("cannot read {}: {e}", path.display())))?;
    let text = String::from_utf8_lossy(&buf);
    let mut all: Vec<&str> = text.lines().collect();
    // The first line of a window that started mid-file is a fragment, not a line.
    if truncated_by_bytes && !all.is_empty() {
        all.remove(0);
    }
    let mut kept: Vec<String> = match grep {
        Some(needle) if !needle.is_empty() => all
            .iter()
            .filter(|l| l.contains(needle))
            .map(|l| (*l).to_string())
            .collect(),
        _ => all.iter().map(|l| (*l).to_string()).collect(),
    };
    let truncated = truncated_by_bytes || kept.len() > lines;
    if kept.len() > lines {
        kept.drain(..kept.len() - lines);
    }
    Ok(Tail {
        lines: kept,
        truncated,
        path: path.to_path_buf(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_log_is_not_an_error() {
        let t = tail(Path::new("/nonexistent/fmonero/bitmonero.log"), 10, None).unwrap();
        assert!(t.lines.is_empty());
        assert!(!t.truncated);
    }

    #[test]
    fn the_tail_keeps_the_last_n_lines_and_filters() {
        let dir = std::env::temp_dir().join(format!("fmonero-log-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bitmonero.log");
        let body: String = (0..50)
            .map(|i| {
                if i % 10 == 0 {
                    format!("{i} ERROR something\n")
                } else {
                    format!("{i} INFO ordinary\n")
                }
            })
            .collect();
        std::fs::write(&path, body).unwrap();
        let t = tail(&path, 5, None).unwrap();
        assert_eq!(t.lines.len(), 5);
        assert!(t.lines[4].starts_with("49 "));
        assert!(t.truncated, "45 lines were dropped");
        let t = tail(&path, 100, Some("ERROR")).unwrap();
        assert_eq!(t.lines.len(), 5);
        assert!(t.lines.iter().all(|l| l.contains("ERROR")));
        assert!(!t.truncated);
        assert_eq!(t.path, path);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

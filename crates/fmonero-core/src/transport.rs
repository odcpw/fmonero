//! One validated wallet-to-daemon transport policy for startup and live configuration.
use crate::{
    settings::{NodeSettings, SslSupport},
    Context, Error,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TlsPolicy {
    #[default]
    Auto,
    Required,
    Disabled,
}
impl std::str::FromStr for TlsPolicy {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Error> {
        match s {
            "auto" => Ok(Self::Auto),
            "required" => Ok(Self::Required),
            "disabled" => Ok(Self::Disabled),
            _ => Err(Error::usage(
                "TLS policy must be auto, required or disabled.",
            )),
        }
    }
}
impl std::fmt::Display for TlsPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Auto => "auto",
            Self::Required => "required",
            Self::Disabled => "disabled",
        })
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct DaemonConnection {
    pub(crate) address: String,
    pub(crate) trusted: bool,
    pub(crate) ssl: SslSupport,
    pub(crate) ca_file: Option<PathBuf>,
    pub(crate) fingerprint: Option<String>,
    pub(crate) proxy: Option<String>,
    #[serde(skip)]
    inherited_proxy: bool,
}
impl DaemonConnection {
    pub fn address(&self) -> &str {
        &self.address
    }
    pub fn trusted(&self) -> bool {
        self.trusted
    }
    pub fn ssl(&self) -> SslSupport {
        self.ssl
    }
    pub fn proxy(&self) -> Option<&str> {
        self.proxy.as_deref()
    }

    pub fn from_context(ctx: &Context) -> Result<Self, Error> {
        let node = &ctx.settings.node;
        let (address, trusted) = if node.mode.is_remote() {
            (node.remote.clone(), node.remote_trusted)
        } else {
            (crate::units::host_port(&node.url), true)
        };
        Self::for_address(node, &address, trusted)
    }

    /// Live configuration accounts for Monero's immutable global --proxy. A
    /// matching owned process is positive evidence that an empty RPC proxy means
    /// inherit that same proxy; there is never a retry without a requested proxy.
    pub fn for_running_wallet(ctx: &Context) -> Result<Self, Error> {
        Self::from_context(ctx)?.with_running_proxy(ctx)
    }

    fn with_running_proxy(mut self, ctx: &Context) -> Result<Self, Error> {
        if let Some(pid) = crate::units::main_pid(ctx, &ctx.settings.wallet.unit)? {
            use std::io::Read;
            use std::os::unix::fs::OpenOptionsExt;
            let mut file = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(format!("/proc/{pid}/cmdline"))
                .map_err(|_| proxy_runtime_error())?;
            let mut bytes = zeroize::Zeroizing::new(Vec::new());
            file.by_ref()
                .take(65537)
                .read_to_end(&mut bytes)
                .map_err(|_| proxy_runtime_error())?;
            if bytes.len() > 65536 {
                return Err(proxy_runtime_error());
            }
            self.inherited_proxy = inherited_proxy(&bytes, self.proxy.as_deref())?;
            // Guard PID reuse/service replacement between the two observations.
            if crate::units::main_pid(ctx, &ctx.settings.wallet.unit)? != Some(pid) {
                return Err(proxy_runtime_error());
            }
        }
        Ok(self)
    }
    pub(crate) fn rpc_proxy(&self) -> &str {
        if self.inherited_proxy {
            ""
        } else {
            self.proxy.as_deref().unwrap_or("")
        }
    }

    pub fn for_address(node: &NodeSettings, address: &str, trusted: bool) -> Result<Self, Error> {
        let address = crate::Settings::parse_remote_address(address)?;
        if node.ca_file.is_some() && node.fingerprint.is_some() {
            return Err(Error::usage("Choose either a certificate authority file or a certificate fingerprint, not both."));
        }
        let fingerprint = node
            .fingerprint
            .as_deref()
            .map(normalize_fingerprint)
            .transpose()?;
        let proxy = node.proxy.as_deref().map(validate_proxy).transpose()?;
        let local = crate::settings::is_loopback_host(crate::settings::remote_host(&address));
        let hidden = crate::settings::remote_host(&address).ends_with(".onion")
            || crate::settings::remote_host(&address).ends_with(".i2p");
        let required = address.starts_with("https://")
            || node.tls == TlsPolicy::Required
            || node.ca_file.is_some()
            || fingerprint.is_some()
            || (proxy.is_some() && !hidden && !local);
        if required && node.tls == TlsPolicy::Disabled {
            return Err(Error::usage("This address or certificate/proxy policy requires TLS. Select required TLS or change the conflicting setting."));
        }
        let ssl = if required {
            SslSupport::Enabled
        } else if node.tls == TlsPolicy::Disabled || local {
            SslSupport::Disabled
        } else {
            SslSupport::Autodetect
        };
        let mut ca_file = node.ca_file.clone();
        if let Some(path) = &ca_file {
            if !path.is_absolute() || path.as_os_str().as_encoded_bytes().iter().any(|b| *b < 32) {
                return Err(Error::usage("The certificate authority file needs an absolute path without control characters."));
            }
            if !path.is_file() {
                return Err(Error::usage(
                    "The certificate authority file does not exist or is not a regular file.",
                ));
            }
        }
        // Monero 0.18.5.3 wallet2::make_basic requires strong verification when
        // either TLS is enabled OR a global proxy is present, even for plaintext
        // loopback. Supply its startup prerequisite without changing `ssl` above.
        // Hidden services supply their own identity; explicit CA/pin wins as usual.
        let verification_required = required || (proxy.is_some() && !hidden);
        if verification_required && ca_file.is_none() && fingerprint.is_none() {
            ca_file = ["/etc/ssl/certs/ca-certificates.crt", "/etc/ssl/cert.pem"]
                .iter()
                .map(PathBuf::from)
                .find(|path| path.is_file());
            if ca_file.is_none() {
                return Err(Error::usage("This connection needs the system CA bundle or an explicit certificate authority file/fingerprint."));
            }
        }
        Ok(Self {
            address,
            trusted,
            ssl,
            ca_file,
            fingerprint,
            proxy,
            inherited_proxy: false,
        })
    }

    /// Public non-secret argv only; callers add their executable and own the process.
    pub fn daemon_args(&self) -> Vec<String> {
        let mut args = vec![
            "--daemon-address".into(),
            self.address.clone(),
            if self.trusted {
                "--trusted-daemon".into()
            } else {
                "--untrusted-daemon".into()
            },
            "--daemon-ssl".into(),
            self.ssl.to_string(),
        ];
        if let Some(path) = &self.ca_file {
            args.extend([
                "--daemon-ssl-ca-certificates".into(),
                path.to_string_lossy().into_owned(),
            ]);
        }
        if let Some(pin) = &self.fingerprint {
            args.extend(["--daemon-ssl-allowed-fingerprints".into(), pin.clone()]);
        }
        if let Some(proxy) = &self.proxy {
            args.extend(["--proxy".into(), proxy.clone()]);
        }
        args
    }
}

fn proxy_runtime_error() -> Error {
    Error::safety("The running wallet's proxy policy cannot be changed or verified. Close the wallet and restart its service with the selected policy before reopening.")
        .with_reason("transport_policy").with_hint("fmonero app stop --yes")
}
fn inherited_proxy(cmdline: &[u8], desired: Option<&str>) -> Result<bool, Error> {
    let args: Vec<&[u8]> = cmdline
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .collect();
    let executable = args
        .first()
        .and_then(|s| std::str::from_utf8(s).ok())
        .ok_or_else(proxy_runtime_error)?;
    if std::path::Path::new(executable)
        .file_name()
        .is_none_or(|s| s != "monero-wallet-rpc")
    {
        return Err(proxy_runtime_error());
    }
    let mut global = None;
    let mut i = 1;
    while i < args.len() {
        let arg = std::str::from_utf8(args[i]).map_err(|_| proxy_runtime_error())?;
        // Our generated unit has no config file. A foreign config could conceal a
        // second/global proxy, and is not evidence for silently inheriting it.
        if arg == "--config-file" || arg.starts_with("--config-file=") {
            return Err(proxy_runtime_error());
        }
        let value = if arg == "--proxy" {
            i += 1;
            Some(
                std::str::from_utf8(args.get(i).ok_or_else(proxy_runtime_error)?)
                    .map_err(|_| proxy_runtime_error())?,
            )
        } else {
            arg.strip_prefix("--proxy=")
        };
        if let Some(value) = value {
            if global.is_some() {
                return Err(proxy_runtime_error());
            }
            global = Some(validate_proxy(value).map_err(|_| proxy_runtime_error())?);
        }
        i += 1;
    }
    match (global.as_deref(), desired) {
        (Some(actual), Some(wanted)) if actual == wanted => Ok(true),
        (None, _) => Ok(false),
        _ => Err(proxy_runtime_error()),
    }
}

pub fn normalize_fingerprint(value: &str) -> Result<String, Error> {
    let normalized = value.replace(':', "").to_ascii_lowercase();
    if normalized.len() != 64 || !normalized.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::usage(
            "A certificate fingerprint must contain exactly 32 SHA-256 bytes as hexadecimal.",
        ));
    }
    Ok(normalized)
}
pub fn validate_proxy(value: &str) -> Result<String, Error> {
    let endpoint: std::net::SocketAddr = value.parse()
        .map_err(|_| Error::usage("Use a SOCKS proxy IP address and port, such as 127.0.0.1:9050. Credentials and hostnames are not accepted."))?;
    if endpoint.port() == 0 {
        return Err(Error::usage(
            "The SOCKS proxy port must be greater than zero.",
        ));
    }
    Ok(endpoint.to_string())
}

pub(crate) fn systemd_arg(arg: &str) -> String {
    if !arg.is_empty()
        && arg
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_./:-[]".contains(&b))
    {
        return arg.to_owned();
    }
    format!(
        "\"{}\"",
        arg.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%")
            .replace('$', "$$")
    )
}

pub fn configure(ctx: &Context, wallet: &crate::WalletClient) -> Result<(), Error> {
    let connection = DaemonConnection::for_running_wallet(ctx)?;
    let login = if ctx.settings.node.mode.is_remote() {
        None
    } else {
        crate::rpc::Login::from_monerod_conf(&ctx.paths.monerod_conf())?
    };
    wallet.set_connection(&connection, login.as_ref())
}

/// Change only the open wallet's daemon, without saving settings or restarting a
/// service. The captured wallet identity, pending records and running Monero's
/// global proxy all remain part of the same guarded mutation.
pub fn configure_once(
    ctx: &Context,
    wallet: &crate::WalletClient,
    expected: &crate::wallet_ops::WalletIdentity,
    address: &str,
    trusted: bool,
) -> Result<DaemonConnection, Error> {
    let connection = DaemonConnection::for_address(&ctx.settings.node, address, trusted)?;
    let _lock = crate::wallet_ops::guard(ctx, wallet, expected)?;
    let connection = connection.with_running_proxy(ctx)?;
    let login =
        if crate::settings::is_loopback_host(crate::settings::remote_host(connection.address())) {
            crate::rpc::Login::from_monerod_conf(&ctx.paths.monerod_conf())?
        } else {
            None
        };
    wallet.set_connection(&connection, login.as_ref())?;
    Ok(connection)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn proxy_startup_supplies_system_ca_without_changing_loopback_tls() {
        for tls in [TlsPolicy::Auto, TlsPolicy::Disabled] {
            let node = NodeSettings {
                tls,
                proxy: Some("127.0.0.1:9050".into()),
                ..Default::default()
            };
            for address in ["127.0.0.1:18081", "http://127.0.0.1:18081", "[::1]:18081"] {
                let connection = DaemonConnection::for_address(&node, address, true).unwrap();
                assert_eq!(connection.ssl(), SslSupport::Disabled);
                let ca = connection
                    .ca_file
                    .as_ref()
                    .expect("proxy startup requires a CA");
                assert!(ca.is_file());
                assert!(connection.fingerprint.is_none());
                let args = connection.daemon_args();
                assert!(args
                    .windows(2)
                    .any(|pair| pair == ["--daemon-ssl", "disabled"]));
                assert!(args.windows(2).any(|pair| {
                    pair[0] == "--daemon-ssl-ca-certificates"
                        && pair[1].as_str() == ca.to_string_lossy().as_ref()
                }));
                assert!(args
                    .windows(2)
                    .any(|pair| pair == ["--proxy", "127.0.0.1:9050"]));
                assert!(!args.iter().any(|arg| arg == "--daemon-ssl-allow-any-cert"));
            }
            let without_proxy = NodeSettings {
                proxy: None,
                ..node
            };
            let connection =
                DaemonConnection::for_address(&without_proxy, "127.0.0.1:18081", true).unwrap();
            assert_eq!(connection.ssl(), SslSupport::Disabled);
            assert!(connection.ca_file.is_none());
        }
    }

    #[test]
    fn proxy_startup_preserves_explicit_verification_and_required_tls() {
        let mut node = NodeSettings {
            proxy: Some("127.0.0.1:9050".into()),
            ..Default::default()
        };
        for address in [
            "https://127.0.0.1:18081",
            "https://node.example:18081",
            "node.example:18081",
        ] {
            let connection = DaemonConnection::for_address(&node, address, false).unwrap();
            assert_eq!(connection.ssl(), SslSupport::Enabled);
            assert!(connection.ca_file.as_ref().unwrap().is_file());
        }
        let connection =
            DaemonConnection::for_address(&node, "https://node.example:18081", false).unwrap();
        // Reuse the real system bundle as an explicitly selected path. Neither
        // selecting a CA nor a pin may introduce a second verification source.
        node.ca_file = connection.ca_file;
        let explicit =
            DaemonConnection::for_address(&node, "https://node.example:18081", false).unwrap();
        assert_eq!(explicit.ca_file, node.ca_file);
        assert!(explicit.fingerprint.is_none());
        node.ca_file = None;
        node.fingerprint = Some("ab".repeat(32));
        let pinned =
            DaemonConnection::for_address(&node, "https://node.example:18081", false).unwrap();
        assert_eq!(pinned.ssl(), SslSupport::Enabled);
        assert_eq!(pinned.fingerprint, node.fingerprint);
        assert!(pinned.ca_file.is_none());
        node.tls = TlsPolicy::Disabled;
        assert!(DaemonConnection::for_address(&node, "https://node.example:18081", false).is_err());
    }

    #[test]
    fn proxy_startup_hidden_hosts_need_no_implicit_ca() {
        for tls in [TlsPolicy::Auto, TlsPolicy::Disabled] {
            let node = NodeSettings {
                tls,
                proxy: Some("127.0.0.1:9050".into()),
                ..Default::default()
            };
            for address in ["walletnode.onion:18081", "walletnode.i2p:18081"] {
                let connection = DaemonConnection::for_address(&node, address, false).unwrap();
                assert_eq!(
                    connection.ssl(),
                    if tls == TlsPolicy::Disabled {
                        SslSupport::Disabled
                    } else {
                        SslSupport::Autodetect
                    }
                );
                assert!(connection.ca_file.is_none());
                assert!(connection.fingerprint.is_none());
                let args = connection.daemon_args();
                assert!(args
                    .windows(2)
                    .any(|pair| pair == ["--proxy", "127.0.0.1:9050"]));
                assert!(!args.iter().any(|arg| arg == "--daemon-ssl-allow-any-cert"));
            }
        }
    }

    #[test]
    fn explicit_https_and_pins_cannot_downgrade() {
        let mut node = NodeSettings {
            fingerprint: Some("ab".repeat(32)),
            ..Default::default()
        };
        let c = DaemonConnection::for_address(&node, "https://node.example:18081", false).unwrap();
        assert_eq!(c.ssl, SslSupport::Enabled);
        assert!(c
            .daemon_args()
            .contains(&"--daemon-ssl-allowed-fingerprints".into()));
        node.tls = TlsPolicy::Disabled;
        assert!(DaemonConnection::for_address(&node, "https://node.example:18081", false).is_err());
        node.tls = TlsPolicy::Auto;
        node.ca_file = Some(PathBuf::from("/missing"));
        assert!(DaemonConnection::for_address(&node, "https://node.example:18081", false).is_err());
    }
    #[test]
    fn global_proxy_inheritance_requires_exact_positive_process_evidence() {
        let process = b"/usr/bin/monero-wallet-rpc\0--proxy\x00127.0.0.1:9050\0";
        assert!(inherited_proxy(process, Some("127.0.0.1:9050")).unwrap());
        assert!(inherited_proxy(process, Some("127.0.0.1:9150")).is_err());
        assert!(inherited_proxy(process, None).is_err());
        assert!(!inherited_proxy(b"monero-wallet-rpc\0", Some("127.0.0.1:9050")).unwrap());
        assert!(inherited_proxy(
            b"other-program\0--proxy=127.0.0.1:9050\0",
            Some("127.0.0.1:9050")
        )
        .is_err());
        assert!(inherited_proxy(b"monero-wallet-rpc\0--config-file=private\0", None).is_err());
        assert!(inherited_proxy(b"monero-wallet-rpc\0--proxy\0", None).is_err());
    }
    #[test]
    fn proxy_rejects_credentials_dns_and_zero_ports() {
        assert_eq!(validate_proxy("127.0.0.1:9050").unwrap(), "127.0.0.1:9050");
        for invalid in [
            "user:pass@127.0.0.1:9050",
            "localhost:9050",
            "127.0.0.1:0",
            "http://127.0.0.1:9050",
        ] {
            assert!(validate_proxy(invalid).is_err());
        }
    }
}

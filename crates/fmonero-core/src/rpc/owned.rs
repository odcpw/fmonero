//! Linux kernel proof of the actual connected wallet peer, before HTTP bytes.
//! Digest authenticates the client only. No listener lookup before connect, PID
//! heuristic or successful get_version response substitutes for this socket proof.
use super::http_support::{
    Buffers, ConnectionDetails, Connector, HttpError, HttpTransport, NextTimeout,
};
use std::{
    io::{self, Read, Write},
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream},
    time::{Duration, Instant},
};
use zeroize::{Zeroize, Zeroizing};

#[derive(Debug)]
pub(super) struct OwnedConnector {
    pub(super) expected_uid: u32,
}
impl Connector for OwnedConnector {
    type Out = OwnedTransport;
    fn connect(
        &self,
        details: &ConnectionDetails,
        _: Option<()>,
    ) -> Result<Option<OwnedTransport>, HttpError> {
        if details.needs_tls()
            || details.config.proxy().is_some()
            || details.addrs.is_empty()
            || details.addrs.iter().any(|a| !a.ip().is_loopback())
        {
            return Err(refused().into());
        }
        let budget = duration(details.timeout)?;
        let start = Instant::now();
        let mut last = None;
        for address in &details.addrs {
            let remaining = budget
                .checked_sub(start.elapsed())
                .filter(|d| !d.is_zero())
                .ok_or(HttpError::Timeout(details.timeout.reason))?;
            let stream = match TcpStream::connect_timeout(address, remaining) {
                Ok(s) => s,
                Err(e) => {
                    last = Some(e);
                    continue;
                }
            };
            // The four-tuple belongs to this live TCP stream. A different user
            // rebinding the port cannot inherit or replace this connection.
            verify_owner(&stream, self.expected_uid, start + budget)?;
            stream.set_nodelay(true)?;
            return Ok(Some(OwnedTransport {
                stream,
                buffers: PrivateBuffers::new(),
            }));
        }
        Err(last
            .unwrap_or_else(|| io::Error::from(io::ErrorKind::ConnectionRefused))
            .into())
    }
}
fn refused() -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, "wallet_peer_unverified")
}
fn duration(timeout: NextTimeout) -> Result<Duration, HttpError> {
    if timeout.after.is_zero() {
        return Err(HttpError::Timeout(timeout.reason));
    }
    let value = timeout
        .not_zero()
        .map(|d| *d)
        .unwrap_or(Duration::from_secs(30));
    if value.is_zero() {
        Err(HttpError::Timeout(timeout.reason))
    } else {
        Ok(value)
    }
}
fn verify_owner(stream: &TcpStream, uid: u32, deadline: Instant) -> io::Result<()> {
    let client = normalize(stream.local_addr()?);
    let server = normalize(stream.peer_addr()?);
    if !client.ip().is_loopback() || !server.ip().is_loopback() {
        return Err(refused());
    }
    // Accepted sockets normally appear immediately. Bound a scheduling delay;
    // never send a request just to make the peer prove that it is alive.
    let deadline = deadline.min(Instant::now() + Duration::from_secs(1));
    loop {
        for path in ["/proc/net/tcp", "/proc/net/tcp6"] {
            let file = match std::fs::File::open(path) {
                Ok(f) => f,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(_) => return Err(refused()),
            };
            let mut table = String::new();
            file.take(4 * 1024 * 1024 + 1)
                .read_to_string(&mut table)
                .map_err(|_| refused())?;
            if table.len() > 4 * 1024 * 1024 {
                return Err(refused());
            }
            if let Some(actual) = owner_in_table(&table, server, client) {
                return if actual == uid {
                    Ok(())
                } else {
                    Err(refused())
                };
            }
        }
        if Instant::now() >= deadline {
            return Err(refused());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn normalize(address: SocketAddr) -> SocketAddr {
    match address.ip() {
        IpAddr::V6(ip) => ip
            .to_ipv4_mapped()
            .map(|v| SocketAddr::new(v.into(), address.port()))
            .unwrap_or(address),
        _ => address,
    }
}
fn endpoint(text: &str) -> Option<SocketAddr> {
    let (hex, port) = text.split_once(':')?;
    let port = u16::from_str_radix(port, 16).ok()?;
    let ip = match hex.len() {
        8 => IpAddr::V4(Ipv4Addr::from(
            u32::from_str_radix(hex, 16).ok()?.to_ne_bytes(),
        )),
        32 => {
            let mut bytes = [0u8; 16];
            for (i, word) in hex.as_bytes().as_chunks::<8>().0.iter().enumerate() {
                let word = u32::from_str_radix(std::str::from_utf8(word).ok()?, 16).ok()?;
                bytes[i * 4..i * 4 + 4].copy_from_slice(&word.to_ne_bytes());
            }
            IpAddr::V6(Ipv6Addr::from(bytes))
        }
        _ => return None,
    };
    Some(normalize(SocketAddr::new(ip, port)))
}
fn owner_in_table(table: &str, server: SocketAddr, client: SocketAddr) -> Option<u32> {
    for line in table.lines().skip(1) {
        let fields: Vec<_> = line.split_ascii_whitespace().take(10).collect();
        if fields.len() != 10 || fields[3] != "01" {
            continue;
        }
        if endpoint(fields[1]) == Some(server)
            && endpoint(fields[2]) == Some(client)
            && fields[9].parse::<u64>().ok().is_some_and(|n| n != 0)
        {
            return fields[7].parse().ok();
        }
    }
    None
}

pub(super) struct OwnedTransport {
    stream: TcpStream,
    buffers: PrivateBuffers,
}
impl std::fmt::Debug for OwnedTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OwnedWalletTransport(<protected>)")
    }
}
impl HttpTransport for OwnedTransport {
    fn buffers(&mut self) -> &mut dyn Buffers {
        &mut self.buffers
    }
    fn transmit_output(&mut self, amount: usize, timeout: NextTimeout) -> Result<(), HttpError> {
        self.stream.set_write_timeout(Some(duration(timeout)?))?;
        let result = self.stream.write_all(&self.buffers.output[..amount]);
        self.buffers.output[..].zeroize();
        self.buffers.tmp[..].zeroize();
        map_io(result, timeout)
    }
    fn await_input(&mut self, timeout: NextTimeout) -> Result<bool, HttpError> {
        self.stream.set_read_timeout(Some(duration(timeout)?))?;
        let buffer = self.buffers.input_append_buf();
        if buffer.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "wallet response header exceeds its buffer",
            )
            .into());
        }
        let n = map_io(self.stream.read(buffer), timeout)?;
        self.buffers.input_appended(n);
        Ok(n > 0)
    }
    fn is_open(&mut self) -> bool {
        if self.stream.set_nonblocking(true).is_err() {
            return false;
        }
        let mut byte = [0];
        let open =
            matches!(self.stream.peek(&mut byte),Err(e) if e.kind()==io::ErrorKind::WouldBlock);
        let reset = self.stream.set_nonblocking(false).is_ok();
        open && reset
    }
}
fn map_io<T>(result: io::Result<T>, timeout: NextTimeout) -> Result<T, HttpError> {
    result.map_err(|e| {
        if matches!(
            e.kind(),
            io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
        ) {
            HttpError::Timeout(timeout.reason)
        } else {
            e.into()
        }
    })
}
struct PrivateBuffers {
    input: Zeroizing<Vec<u8>>,
    output: Zeroizing<Vec<u8>>,
    tmp: Zeroizing<Vec<u8>>,
    filled: usize,
    progress: bool,
}
impl PrivateBuffers {
    fn new() -> Self {
        Self {
            input: Zeroizing::new(vec![0; 128 * 1024]),
            output: Zeroizing::new(vec![0; 16 * 1024]),
            tmp: Zeroizing::new(vec![0; 16 * 1024]),
            filled: 0,
            progress: false,
        }
    }
}
impl Buffers for PrivateBuffers {
    fn output(&mut self) -> &mut [u8] {
        &mut self.output
    }
    fn input(&self) -> &[u8] {
        &self.input[..self.filled]
    }
    fn input_append_buf(&mut self) -> &mut [u8] {
        &mut self.input[self.filled..]
    }
    fn input_appended(&mut self, amount: usize) {
        self.filled += amount;
    }
    fn input_consume(&mut self, amount: usize) {
        self.progress = amount > 0;
        if amount > 0 {
            let old = self.filled;
            self.input.copy_within(amount..old, 0);
            self.filled -= amount;
            self.input[self.filled..old].zeroize();
        }
    }
    fn tmp_and_output(&mut self) -> (&mut [u8], &mut [u8]) {
        (&mut self.tmp, &mut self.output)
    }
    fn can_use_input(&self) -> bool {
        self.filled > 0 && self.progress
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn kernel_proof_accepts_current_user_and_refuses_other_uid_without_sending() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut server, _) = listener.accept().unwrap();
        let uid = unsafe { libc::geteuid() };
        verify_owner(&client, uid, Instant::now() + Duration::from_secs(1)).unwrap();
        assert!(verify_owner(
            &client,
            uid.wrapping_add(1),
            Instant::now() + Duration::from_secs(1)
        )
        .is_err());
        server.set_nonblocking(true).unwrap();
        let mut byte = [0];
        assert_eq!(
            server.read(&mut byte).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
    }
    #[test]
    fn socket_proof_requires_established_reverse_tuple_and_inode() {
        let server = "127.0.0.1:18083".parse().unwrap();
        let client = "127.0.0.1:40123".parse().unwrap();
        let row="header\n 0: 0100007F:46A3 0100007F:9CBB 01 00000000:00000000 00:00000000 00000000 1234 0 42\n";
        assert_eq!(owner_in_table(row, server, client), Some(1234));
        assert_eq!(owner_in_table(row, client, server), None);
        assert_eq!(
            owner_in_table(&row.replace(" 01 ", " 0A "), server, client),
            None
        );
        assert_eq!(
            owner_in_table(&row.replace(" 0 42", " 0 0"), server, client),
            None
        );
    }
    #[test]
    fn ipv6_and_mapped_kernel_endpoints_preserve_the_full_socket_identity() {
        assert_eq!(
            endpoint("00000000000000000000000001000000:46A3"),
            Some("[::1]:18083".parse().unwrap())
        );
        assert_eq!(
            endpoint("0000000000000000FFFF00000100007F:46A3"),
            Some("127.0.0.1:18083".parse().unwrap())
        );
        assert!(endpoint("0100007F:not-a-port").is_none());
    }
}

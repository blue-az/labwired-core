//! Virtual AP internet egress — DNS + TCP NAT so a station on the modeled AP
//! can reach the real host network (native only).
//!
//! The station sees a normal SoftAP: DHCP gives gateway+DNS = AP IP. Off-LAN
//! traffic is ARPed to the AP and NATed here:
//!   * UDP/53 to the AP → minimal recursive DNS (host resolver)
//!   * TCP to a non-local IP → non-blocking `TcpStream` shuttle
//!
//! Browser wasm has no sockets: [`internet_enabled`] is false and external
//! traffic is dropped (the playground still injects live public-stats for the
//! stats lab). Set `LABWIRED_WIFI_NO_INTERNET=1` to force offline on native.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

/// When false, external destinations are dropped (wasm / offline CI).
pub fn internet_enabled() -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        return false;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::env::var_os("LABWIRED_WIFI_NO_INTERNET").is_none()
            && std::env::var_os("LABWIRED_WIFI_STATS_OFFLINE").is_none()
    }
}

/// Per-station TCP connection through the NAT (key = station source port).
pub struct EgressTcp {
    pub rcv_nxt: u32,
    pub snd_nxt: u32,
    pub fin_sent: bool,
    pub client_ip: [u8; 4],
    pub remote_ip: [u8; 4],
    pub remote_port: u16,
    stream: Option<TcpStream>,
    /// Remote peer closed its write half.
    pub peer_fin: bool,
}

impl std::fmt::Debug for EgressTcp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EgressTcp")
            .field("client_ip", &self.client_ip)
            .field("remote_ip", &self.remote_ip)
            .field("remote_port", &self.remote_port)
            .field("fin_sent", &self.fin_sent)
            .field("has_stream", &self.stream.is_some())
            .finish()
    }
}

impl EgressTcp {
    /// Open a real TCP connection to `remote_ip:remote_port` (blocking connect
    /// with a short timeout, then non-blocking I/O).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn connect(
        client_ip: [u8; 4],
        remote_ip: [u8; 4],
        remote_port: u16,
        rcv_nxt: u32,
        snd_nxt: u32,
    ) -> Option<Self> {
        let addr = SocketAddr::from((remote_ip, remote_port));
        let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(3)).ok()?;
        let _ = stream.set_nonblocking(true);
        let _ = stream.set_nodelay(true);
        Some(Self {
            rcv_nxt,
            snd_nxt,
            fin_sent: false,
            client_ip,
            remote_ip,
            remote_port,
            stream: Some(stream),
            peer_fin: false,
        })
    }

    #[cfg(target_arch = "wasm32")]
    pub fn connect(
        _client_ip: [u8; 4],
        _remote_ip: [u8; 4],
        _remote_port: u16,
        _rcv_nxt: u32,
        _snd_nxt: u32,
    ) -> Option<Self> {
        None
    }

    pub fn write_all(&mut self, data: &[u8]) -> bool {
        let Some(stream) = self.stream.as_mut() else {
            return false;
        };
        // Prefer a complete write; fall back to best-effort for non-blocking.
        match stream.write_all(data) {
            Ok(()) => true,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                let _ = stream.write(data);
                true
            }
            Err(_) => false,
        }
    }

    /// Read available bytes from the real peer (up to `cap`).
    pub fn read_available(&mut self, cap: usize) -> Vec<u8> {
        let Some(stream) = self.stream.as_mut() else {
            return Vec::new();
        };
        let mut buf = vec![0u8; cap.min(4096)];
        match stream.read(&mut buf) {
            Ok(0) => {
                self.peer_fin = true;
                Vec::new()
            }
            Ok(n) => {
                buf.truncate(n);
                buf
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Vec::new(),
            Err(_) => {
                self.peer_fin = true;
                Vec::new()
            }
        }
    }

    pub fn shutdown_write(&mut self) {
        if let Some(stream) = self.stream.as_mut() {
            let _ = stream.shutdown(Shutdown::Write);
        }
    }
}

/// Resolve a hostname (or dotted-quad) to IPv4 addresses via the host resolver.
pub fn resolve_a(name: &str) -> Vec<[u8; 4]> {
    if !internet_enabled() {
        return Vec::new();
    }
    // Dotted quad short-circuit.
    if let Ok(ip) = name.parse::<std::net::Ipv4Addr>() {
        return vec![ip.octets()];
    }
    let host_port = format!("{name}:0");
    match host_port.to_socket_addrs() {
        Ok(iter) => iter
            .filter_map(|a| match a {
                SocketAddr::V4(v4) => Some(v4.ip().octets()),
                _ => None,
            })
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// If `udp_payload` is a DNS query asking for A records, build a response
/// using the host resolver. Returns `None` if not a parseable A query.
pub fn dns_respond(udp_payload: &[u8]) -> Option<Vec<u8>> {
    if udp_payload.len() < 12 {
        return None;
    }
    let id = &udp_payload[0..2];
    let flags = u16::from_be_bytes([udp_payload[2], udp_payload[3]]);
    // QR must be 0 (query).
    if flags & 0x8000 != 0 {
        return None;
    }
    let qdcount = u16::from_be_bytes([udp_payload[4], udp_payload[5]]);
    if qdcount != 1 {
        return None;
    }
    // Parse QNAME
    let mut i = 12usize;
    let mut labels = Vec::new();
    while i < udp_payload.len() {
        let len = udp_payload[i] as usize;
        if len == 0 {
            i += 1;
            break;
        }
        if len & 0xC0 == 0xC0 {
            return None; // compression in query — rare, skip
        }
        i += 1;
        if i + len > udp_payload.len() {
            return None;
        }
        labels.push(
            std::str::from_utf8(&udp_payload[i..i + len])
                .ok()?
                .to_string(),
        );
        i += len;
    }
    if i + 4 > udp_payload.len() {
        return None;
    }
    let qtype = u16::from_be_bytes([udp_payload[i], udp_payload[i + 1]]);
    let qclass = u16::from_be_bytes([udp_payload[i + 2], udp_payload[i + 3]]);
    i += 4;
    if qclass != 1 {
        return None; // IN
    }
    // Only A (1). AAAA (28) → empty NOERROR so clients fall back.
    let name = labels.join(".");
    let answers: Vec<[u8; 4]> = if qtype == 1 {
        resolve_a(&name)
    } else {
        Vec::new()
    };

    let mut out = Vec::new();
    out.extend_from_slice(id);
    // QR=1, AA=1, RD copied, RA=1
    let rd = flags & 0x0100;
    let rcode: u16 = if answers.is_empty() && qtype == 1 { 3 } else { 0 }; // NXDOMAIN if no A
    let rflags = 0x8000 | 0x0400 | rd | 0x0080 | rcode;
    out.extend_from_slice(&rflags.to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT
    out.extend_from_slice(&(answers.len() as u16).to_be_bytes()); // ANCOUNT
    out.extend_from_slice(&0u16.to_be_bytes()); // NSCOUNT
    out.extend_from_slice(&0u16.to_be_bytes()); // ARCOUNT
    // Question section copy
    out.extend_from_slice(&udp_payload[12..i]);
    // Answers: pointer to name at offset 12
    for ip in answers {
        out.extend_from_slice(&[0xC0, 0x0C]); // compression pointer
        out.extend_from_slice(&1u16.to_be_bytes()); // TYPE A
        out.extend_from_slice(&1u16.to_be_bytes()); // CLASS IN
        out.extend_from_slice(&60u32.to_be_bytes()); // TTL 60s
        out.extend_from_slice(&4u16.to_be_bytes()); // RDLENGTH
        out.extend_from_slice(&ip);
    }
    Some(out)
}

/// Key for egress map: station MAC + client TCP port.
pub type EgressKey = ([u8; 6], u16);

/// Table of open NAT connections.
#[derive(Debug, Default)]
pub struct EgressTable {
    pub conns: HashMap<EgressKey, EgressTcp>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_dotted_quad() {
        let ips = resolve_a("127.0.0.1");
        if internet_enabled() {
            assert_eq!(ips, vec![[127, 0, 0, 1]]);
        }
    }

    #[test]
    fn dns_respond_builds_a_record() {
        // Minimal DNS query for example.com A (pre-encoded).
        // We'll build programmatically:
        let mut q = Vec::new();
        q.extend_from_slice(&[0x12, 0x34]); // id
        q.extend_from_slice(&[0x01, 0x00]); // RD
        q.extend_from_slice(&[0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        for label in ["example", "com"] {
            q.push(label.len() as u8);
            q.extend_from_slice(label.as_bytes());
        }
        q.push(0);
        q.extend_from_slice(&[0x00, 0x01, 0x00, 0x01]); // A IN
        let resp = dns_respond(&q);
        if !internet_enabled() {
            return;
        }
        let resp = resp.expect("dns response");
        assert_eq!(&resp[0..2], &[0x12, 0x34]);
        assert!(resp[2] & 0x80 != 0, "QR set");
    }
}

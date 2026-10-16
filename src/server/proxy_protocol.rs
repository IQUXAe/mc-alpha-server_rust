//! HAProxy PROXY protocol v1 and v2 parser.
//!
//! Specification: https://www.haproxy.org/download/1.8/doc/proxy-protocol.txt
//!
//! Provides zero-copy parsing of incoming TCP connection headers sent by
//! reverse proxies like HAProxy, rathole, NGINX, or cloud load balancers.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

/// PROXY protocol v2 12-byte signature: `\r\n\r\n\0\r\nQUIT\n`
pub const V2_SIG: [u8; 12] = [
    0x0D, 0x0A, 0x0D, 0x0A, 0x00, 0x0D, 0x0A, 0x51, 0x55, 0x49, 0x54, 0x0A,
];

/// PROXY protocol v1 signature prefix: `"PROXY "`
pub const V1_SIG: &[u8; 6] = b"PROXY ";

/// Result of attempting to parse a PROXY protocol header.
#[derive(Debug, PartialEq, Eq)]
pub enum ProxyHeaderResult {
    /// Full header parsed successfully.
    /// `consumed`: number of bytes to drain from the front of the receive buffer.
    /// `client_addr`: the parsed original client address (if available).
    Complete {
        consumed: usize,
        client_addr: Option<SocketAddr>,
    },
    /// The prefix matches a PROXY header, but more bytes are needed from the socket.
    Incomplete,
    /// The buffer does NOT match any PROXY header (e.g. direct Minecraft client packet).
    NotProxyHeader,
    /// The header was identified as a PROXY header but is corrupt or invalid.
    InvalidHeader,
}

/// Parse a PROXY protocol v1 or v2 header from the beginning of `buf`.
pub fn parse_proxy_header(buf: &[u8]) -> ProxyHeaderResult {
    if buf.is_empty() {
        return ProxyHeaderResult::Incomplete;
    }

    // Check v2 binary header first
    if buf.len() >= 12 && buf.starts_with(&V2_SIG) {
        return parse_v2(buf);
    }
    if buf.len() < 12 && V2_SIG.starts_with(buf) {
        return ProxyHeaderResult::Incomplete;
    }

    // Check v1 text header
    if buf.len() >= 6 && buf.starts_with(V1_SIG) {
        return parse_v1(buf);
    }
    if buf.len() < 6 && V1_SIG.starts_with(buf) {
        return ProxyHeaderResult::Incomplete;
    }

    ProxyHeaderResult::NotProxyHeader
}

fn parse_v1(buf: &[u8]) -> ProxyHeaderResult {
    // Look for "\r\n"
    let crlf_pos = match buf.windows(2).position(|w| w == b"\r\n") {
        Some(pos) => pos,
        None => {
            // According to spec, maximum length of a v1 header is 107 characters.
            if buf.len() > 107 {
                return ProxyHeaderResult::InvalidHeader;
            }
            return ProxyHeaderResult::Incomplete;
        }
    };

    let consumed = crlf_pos + 2;
    let Ok(line) = std::str::from_utf8(&buf[6..crlf_pos]) else {
        return ProxyHeaderResult::InvalidHeader;
    };

    let mut parts = line.split_ascii_whitespace();
    let proto = match parts.next() {
        Some(p) => p,
        None => return ProxyHeaderResult::InvalidHeader,
    };

    if proto == "UNKNOWN" {
        return ProxyHeaderResult::Complete {
            consumed,
            client_addr: None,
        };
    }

    if proto != "TCP4" && proto != "TCP6" {
        return ProxyHeaderResult::InvalidHeader;
    }

    let src_ip_str = match parts.next() {
        Some(p) => p,
        None => return ProxyHeaderResult::InvalidHeader,
    };
    let _dst_ip_str = match parts.next() {
        Some(p) => p,
        None => return ProxyHeaderResult::InvalidHeader,
    };
    let src_port_str = match parts.next() {
        Some(p) => p,
        None => return ProxyHeaderResult::InvalidHeader,
    };
    let _dst_port_str = match parts.next() {
        Some(p) => p,
        None => return ProxyHeaderResult::InvalidHeader,
    };

    let Ok(ip) = src_ip_str.parse::<IpAddr>() else {
        return ProxyHeaderResult::InvalidHeader;
    };
    let Ok(port) = src_port_str.parse::<u16>() else {
        return ProxyHeaderResult::InvalidHeader;
    };

    match (proto, &ip) {
        ("TCP4", IpAddr::V4(_)) => {}
        ("TCP6", IpAddr::V6(_)) => {}
        _ => return ProxyHeaderResult::InvalidHeader,
    }

    ProxyHeaderResult::Complete {
        consumed,
        client_addr: Some(SocketAddr::new(ip, port)),
    }
}

fn parse_v2(buf: &[u8]) -> ProxyHeaderResult {
    if buf.len() < 16 {
        return ProxyHeaderResult::Incomplete;
    }

    let ver_cmd = buf[12];
    let version = ver_cmd >> 4;
    if version != 2 {
        return ProxyHeaderResult::InvalidHeader;
    }

    let cmd = ver_cmd & 0x0F;
    if cmd != 0x00 && cmd != 0x01 {
        return ProxyHeaderResult::InvalidHeader;
    }

    let fam_proto = buf[13];
    let payload_len = u16::from_be_bytes([buf[14], buf[15]]) as usize;
    let total_len = 16 + payload_len;

    if buf.len() < total_len {
        return ProxyHeaderResult::Incomplete;
    }

    // Command 0x00: LOCAL connection (e.g. health check from proxy)
    if cmd == 0x00 {
        return ProxyHeaderResult::Complete {
            consumed: total_len,
            client_addr: None,
        };
    }

    // Command 0x01: PROXY forwarded connection
    let client_addr = match fam_proto {
        0x11 => {
            // AF_INET (IPv4) + STREAM (TCP)
            if payload_len < 12 {
                return ProxyHeaderResult::InvalidHeader;
            }
            let src_ip = Ipv4Addr::new(buf[16], buf[17], buf[18], buf[19]);
            let src_port = u16::from_be_bytes([buf[24], buf[25]]);
            Some(SocketAddr::new(IpAddr::V4(src_ip), src_port))
        }
        0x21 => {
            // AF_INET6 (IPv6) + STREAM (TCP)
            if payload_len < 36 {
                return ProxyHeaderResult::InvalidHeader;
            }
            let mut ip_bytes = [0u8; 16];
            ip_bytes.copy_from_slice(&buf[16..32]);
            let src_ip = Ipv6Addr::from(ip_bytes);
            let src_port = u16::from_be_bytes([buf[48], buf[49]]);
            Some(SocketAddr::new(IpAddr::V6(src_ip), src_port))
        }
        _ => {
            // UNSPEC, UNIX, or DGRAM: address info is omitted or unsupported
            None
        }
    };

    ProxyHeaderResult::Complete {
        consumed: total_len,
        client_addr,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_v1_tcp4_valid() {
        let mut data = b"PROXY TCP4 203.0.113.195 198.51.100.1 56324 25565\r\nHello".to_vec();
        match parse_proxy_header(&data) {
            ProxyHeaderResult::Complete { consumed, client_addr } => {
                assert_eq!(consumed, 51);
                assert_eq!(
                    client_addr,
                    Some("203.0.113.195:56324".parse::<SocketAddr>().unwrap())
                );
                data.drain(..consumed);
                assert_eq!(&data, b"Hello");
            }
            other => panic!("expected Complete, got {:?}", other),
        }
    }

    #[test]
    fn test_v1_tcp6_valid() {
        let data = b"PROXY TCP6 2001:db8::1 2001:db8::2 56324 25565\r\n";
        match parse_proxy_header(data) {
            ProxyHeaderResult::Complete { consumed, client_addr } => {
                assert_eq!(consumed, data.len());
                assert_eq!(
                    client_addr,
                    Some("[2001:db8::1]:56324".parse::<SocketAddr>().unwrap())
                );
            }
            other => panic!("expected Complete, got {:?}", other),
        }
    }

    #[test]
    fn test_v1_unknown() {
        let data = b"PROXY UNKNOWN\r\n";
        match parse_proxy_header(data) {
            ProxyHeaderResult::Complete { consumed, client_addr } => {
                assert_eq!(consumed, data.len());
                assert_eq!(client_addr, None);
            }
            other => panic!("expected Complete, got {:?}", other),
        }
    }

    #[test]
    fn test_v1_partial_incomplete() {
        assert_eq!(parse_proxy_header(b"PRO"), ProxyHeaderResult::Incomplete);
        assert_eq!(
            parse_proxy_header(b"PROXY TCP4 1.2.3.4 5.6.7.8 1234"),
            ProxyHeaderResult::Incomplete
        );
    }

    #[test]
    fn test_v1_malformed_overlong() {
        let long = vec![b'A'; 120];
        let mut header = b"PROXY ".to_vec();
        header.extend_from_slice(&long);
        assert_eq!(parse_proxy_header(&header), ProxyHeaderResult::InvalidHeader);
    }

    #[test]
    fn test_v2_ipv4_valid() {
        let mut data = Vec::new();
        data.extend_from_slice(&V2_SIG);
        data.push(0x21); // version 2, PROXY
        data.push(0x11); // AF_INET, STREAM
        data.extend_from_slice(&12u16.to_be_bytes()); // length
        // src_ip: 192.168.1.100
        data.extend_from_slice(&[192, 168, 1, 100]);
        // dst_ip: 10.0.0.1
        data.extend_from_slice(&[10, 0, 0, 1]);
        // src_port: 54321
        data.extend_from_slice(&54321u16.to_be_bytes());
        // dst_port: 25565
        data.extend_from_slice(&25565u16.to_be_bytes());
        // Trailing Minecraft payload
        data.extend_from_slice(b"\x02\x00\x04test");

        match parse_proxy_header(&data) {
            ProxyHeaderResult::Complete { consumed, client_addr } => {
                assert_eq!(consumed, 28);
                assert_eq!(
                    client_addr,
                    Some("192.168.1.100:54321".parse::<SocketAddr>().unwrap())
                );
                data.drain(..consumed);
                assert_eq!(&data, b"\x02\x00\x04test");
            }
            other => panic!("expected Complete, got {:?}", other),
        }
    }

    #[test]
    fn test_v2_local_and_tlv() {
        let mut data = Vec::new();
        data.extend_from_slice(&V2_SIG);
        data.push(0x20); // version 2, LOCAL
        data.push(0x00); // UNSPEC
        data.extend_from_slice(&4u16.to_be_bytes()); // length: 4 bytes of TLV/dummy
        data.extend_from_slice(&[0, 1, 2, 3]);

        match parse_proxy_header(&data) {
            ProxyHeaderResult::Complete { consumed, client_addr } => {
                assert_eq!(consumed, 20);
                assert_eq!(client_addr, None);
            }
            other => panic!("expected Complete, got {:?}", other),
        }
    }

    #[test]
    fn test_minecraft_packet_not_proxy() {
        // Minecraft Alpha 1.2.6 Handshake packet: 0x02 followed by username
        let mc_data = b"\x02\x00\x06Player";
        assert_eq!(parse_proxy_header(mc_data), ProxyHeaderResult::NotProxyHeader);
    }
}

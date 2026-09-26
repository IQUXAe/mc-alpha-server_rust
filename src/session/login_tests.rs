
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::time::Duration;
    use crate::network::put_u8;
    use crate::session::transport::{Conn, ConnEvent, bind_listener};

    fn login_packet(protocol: i32, username: &str) -> PacketData {
        PacketData::Login {
            protocol_version: protocol,
            username: username.to_string(),
            password: String::new(),
            map_seed: 0,
            dimension: 0,
        }
    }

    fn read_msg(stream: &mut TcpStream) -> (u8, Vec<u8>) {
        let mut id = [0u8; 1];
        stream.read_exact(&mut id).unwrap();
        let mut len = [0u8; 2];
        stream.read_exact(&mut len).unwrap();
        let len = u16::from_be_bytes(len) as usize;
        let mut body = vec![0u8; len];
        stream.read_exact(&mut body).unwrap();
        (id[0], body)
    }

    fn write_login(stream: &mut TcpStream, protocol: i32, username: &str) {
        let mut b = Vec::new();
        put_u8(&mut b, 1);
        crate::network::put_i32(&mut b, protocol);
        crate::network::put_str(&mut b, username);
        crate::network::put_str(&mut b, "");
        crate::network::put_i64(&mut b, 0);
        crate::network::put_i8(&mut b, 0);
        stream.write_all(&b).unwrap();
    }

    #[test]
    fn test_offline_login_accepts_and_trims() {
        let mut s = LoginSession::new(false);
        s.on_packet(PacketData::Handshake { username: "Steve".to_string() });
        assert_eq!(s.outbox.len(), 1);
        assert_eq!(s.outbox[0][0], 2);
        s.on_packet(login_packet(6, "Steve  \n"));
        match s.poll() {
            Some(LoginEvent::Accepted { username }) => assert_eq!(username, "Steve"),
            other => panic!("expected accept, got {}", other.is_some()),
        }
        assert!(s.poll().is_none());
    }

    #[test]
    fn test_protocol_mismatch_kicks() {
        let mut s = LoginSession::new(false);
        s.on_packet(PacketData::Handshake { username: "Steve".to_string() });
        s.outbox.clear();
        s.on_packet(login_packet(5, "Steve"));
        assert_eq!(s.outbox.len(), 1);
        assert_eq!(s.outbox[0][0], 255);
        match s.poll() {
            Some(LoginEvent::Done) => {}
            _ => panic!("expected done"),
        }
        let mut s = LoginSession::new(false);
        s.on_packet(PacketData::Handshake { username: "Steve".to_string() });
        s.outbox.clear();
        s.on_packet(login_packet(7, "Steve"));
        assert_eq!(s.outbox[0][0], 255);
    }

    #[test]
    fn test_login_timeout_kicks() {
        let mut s = LoginSession::new(false);
        for _ in 0..599 {
            assert!(s.poll().is_none());
        }
        match s.poll() {
            Some(LoginEvent::Done) => {}
            _ => panic!("expected timeout done"),
        }
        assert_eq!(s.outbox.len(), 1);
        assert_eq!(s.outbox[0][0], 255);
    }

    #[test]
    fn test_online_verify_yes_and_no() {
        let yes: VerifyFn = Box::new(|_, _| Ok("YES".to_string()));
        let mut s = LoginSession::new(true).with_verify(yes);
        s.on_packet(PacketData::Handshake { username: "Steve".to_string() });
        assert_eq!(s.outbox[0][0], 2);
        assert_ne!(&s.outbox[0][3..], b"-");
        s.on_packet(login_packet(6, "Steve"));
        let mut ev = None;
        for _ in 0..100 {
            if let Some(e) = s.poll() {
                ev = Some(e);
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        match ev {
            Some(LoginEvent::Accepted { username }) => assert_eq!(username, "Steve"),
            _ => panic!("expected verified accept"),
        }

        let no: VerifyFn = Box::new(|_, _| Ok("NO".to_string()));
        let mut s = LoginSession::new(true).with_verify(no);
        s.on_packet(PacketData::Handshake { username: "Steve".to_string() });
        s.on_packet(login_packet(6, "Steve"));
        let mut ev = None;
        for _ in 0..100 {
            if let Some(e) = s.poll() {
                ev = Some(e);
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        match ev {
            Some(LoginEvent::Done) => {}
            _ => panic!("expected verified kick"),
        }
        assert_eq!(s.outbox.last().unwrap()[0], 255);
    }

    #[test]
    fn test_online_duplicate_login_kicks() {
        let slow: VerifyFn = Box::new(|_, _| {
            std::thread::sleep(Duration::from_millis(200));
            Ok("YES".to_string())
        });
        let mut s = LoginSession::new(true).with_verify(slow);
        s.on_packet(PacketData::Handshake { username: "Steve".to_string() });
        s.on_packet(login_packet(6, "Steve"));
        s.on_packet(login_packet(6, "Steve"));
        match s.poll() {
            Some(LoginEvent::Done) => {}
            _ => panic!("expected duplicate kick"),
        }
    }

    #[test]
    fn test_conn_close_unblocks_reader() {
        let listener = bind_listener("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            Conn::new(stream).unwrap()
        });
        let client = TcpStream::connect(addr).unwrap();
        client.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let conn = handle.join().unwrap();
        conn.close();
        let mut saw_drop = false;
        for _ in 0..100 {
            if conn.drain().iter().any(|e| matches!(e, ConnEvent::Dropped)) {
                saw_drop = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(saw_drop);
    }

    #[test]
    fn test_conn_loopback() {
        let listener = bind_listener("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            Conn::new(stream).unwrap()
        });
        let mut client = TcpStream::connect(addr).unwrap();
        client.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        // Client -> server: handshake.
        let mut hb = Vec::new();
        put_u8(&mut hb, 2);
        crate::network::put_str(&mut hb, "Steve");
        client.write_all(&hb).unwrap();
        let conn = handle.join().unwrap();
        std::thread::sleep(Duration::from_millis(100));
        let events = conn.drain();
        assert!(events.iter().any(|e| matches!(
            e,
            ConnEvent::Packet(PacketData::Handshake { username }) if username == "Steve"
        )));
        // Server -> client: kick bytes arrive intact.
        conn.send(pkt_kick("Bye"));
        assert_eq!(read_msg(&mut client), (255, b"Bye".to_vec()));
        // Client drop surfaces as Dropped.
        drop(client);
        let mut saw_drop = false;
        for _ in 0..100 {
            if conn.drain().iter().any(|e| matches!(e, ConnEvent::Dropped)) {
                saw_drop = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(saw_drop);
    }

    #[test]
    fn test_conn_full_login_over_socket() {
        let listener = bind_listener("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            Conn::new(stream).unwrap()
        });
        let mut client = TcpStream::connect(addr).unwrap();
        client.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let conn = handle.join().unwrap();
        let mut login = LoginSession::new(false);
        // Handshake round trip.
        let mut hb = Vec::new();
        put_u8(&mut hb, 2);
        crate::network::put_str(&mut hb, "Alex");
        client.write_all(&hb).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        for ev in conn.drain() {
            if let ConnEvent::Packet(p) = ev {
                login.on_packet(p);
            }
        }
        for msg in login.outbox.drain(..) {
            conn.send(msg);
        }
        assert_eq!(read_msg(&mut client).1, b"-".to_vec());
        // Login round trip.
        write_login(&mut client, 6, "Alex");
        std::thread::sleep(Duration::from_millis(100));
        for ev in conn.drain() {
            if let ConnEvent::Packet(p) = ev {
                login.on_packet(p);
            }
        }
        match login.poll() {
            Some(LoginEvent::Accepted { username }) => assert_eq!(username, "Alex"),
            _ => panic!("expected accept over socket"),
        }
    }

    #[test]
    fn test_login_rejects_malformed_usernames_before_verify() {
        let mut s = LoginSession::new(true);
        s.on_packet(PacketData::Handshake { username: "-".to_string() });
        // Invalid length (> 16)
        s.on_packet(login_packet(6, "VeryLongUsernameExceedingLimit"));
        assert!(s.poll().is_some());
        assert_eq!(s.outbox.last().unwrap()[0], 255);

        let mut s2 = LoginSession::new(true);
        s2.on_packet(PacketData::Handshake { username: "-".to_string() });
        // Invalid characters (slashes / spaces)
        s2.on_packet(login_packet(6, "../badname"));
        assert!(s2.poll().is_some());
        assert_eq!(s2.outbox.last().unwrap()[0], 255);
    }

    #[test]
    fn test_custom_auth_url_verifier() {
        let url_template = "http://127.0.0.1:9999/check?user={user}&serverId={serverId}".to_string();
        let s = LoginSession::new(true).with_auth_url(url_template);
        assert!(s.verify.is_some());
    }

    #[test]
    fn rejects_duplicate_handshake_and_unexpected_packets() {
        // 1. Login before Handshake must be rejected with Protocol error.
        let mut s = LoginSession::new(true);
        s.on_packet(login_packet(6, "Steve"));
        assert!(matches!(s.poll(), Some(LoginEvent::Done)));
        assert_eq!(s.outbox.last().unwrap(), &pkt_kick("Protocol error"));

        // 2. Duplicate Handshake packet kicks with Protocol error.
        let mut s2 = LoginSession::new(false);
        s2.on_packet(PacketData::Handshake { username: "Steve".to_string() });
        s2.on_packet(PacketData::Handshake { username: "Steve".to_string() });
        assert!(matches!(s2.poll(), Some(LoginEvent::Done)));
        assert_eq!(s2.outbox.last().unwrap(), &pkt_kick("Protocol error"));

        // 3. Unexpected non-login packet (e.g. Chat) during login kicks with Protocol error.
        let mut s3 = LoginSession::new(false);
        s3.on_packet(PacketData::Chat { message: "hi".to_string() });
        assert!(matches!(s3.poll(), Some(LoginEvent::Done)));
        assert_eq!(s3.outbox.last().unwrap(), &pkt_kick("Protocol error"));
    }


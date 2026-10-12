//! Socket transport: non-blocking `mio` TCP connection and packet framing.

use std::cell::RefCell;
use std::io::{Read, Write};
use std::net::TcpListener;
use mio::net::TcpStream;
use crate::network::{PacketData, try_decode_packet_slice};
use crate::server_constants::SEND_QUEUE_MAX_BYTES;

/// Max packets decoded from one socket buffer per poll. Legit clients
/// send a handful per tick; without a cap a 1MB buffer of 1-byte packets
/// decodes into ~1M events before the per-tick kick in `sessions` runs.
/// Leftover bytes stay in `recv_buf` for the next poll.
pub const MAX_PACKETS_PER_POLL: usize = 256;

/// Receive buffer cap while in the login handshake (pre-auth): login
/// traffic is a few dozen bytes, so an unauthenticated socket may not
/// pin more than this in heap memory. The old code reused the 1 MB
/// *send* cap, letting a 256-connection flood pin hundreds of MB with
/// no credentials.
pub const RECV_QUEUE_LOGIN_MAX_BYTES: usize = 16 * 1024;
/// Receive buffer cap post-auth: the largest legit inbound packet
/// (Packet59 sign/chest NBT) is ~65.5 KB, so 128 KB leaves headroom
/// for a burst of small packets sharing the buffer.
pub const RECV_QUEUE_MAX_BYTES: usize = 128 * 1024;

/// Grace window for draining kick bytes of a removed session, in ticks.
pub const CLOSING_SESSION_MAX_TICKS: u32 = 100;

/// Connection event: a decoded packet or a dead socket.
pub enum ConnEvent {
    Packet(PacketData),
    Dropped,
}

struct ConnInner {
    stream: TcpStream,
    recv_buf: Vec<u8>,
    outbound: Vec<u8>,
    inbound: Vec<ConnEvent>,
    recv_limit: usize,
    closed: bool,
    closing: bool,
    dropped_reported: bool,
}

impl ConnInner {
    fn poll_read(&mut self) {
        if self.closed {
            return;
        }
        let mut buf = [0u8; 8192];
        loop {
            match self.stream.read(&mut buf) {
                Ok(0) => {
                    self.closed = true;
                    if !self.dropped_reported {
                        self.dropped_reported = true;
                        self.inbound.push(ConnEvent::Dropped);
                    }
                    return;
                }
                Ok(n) => {
                    self.recv_buf.extend_from_slice(&buf[..n]);
                    if self.recv_buf.len() > self.recv_limit {
                        self.closed = true;
                        if !self.dropped_reported {
                            self.dropped_reported = true;
                            self.inbound.push(ConnEvent::Dropped);
                        }
                        return;
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    break;
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => {
                    continue;
                }
                Err(_) => {
                    self.closed = true;
                    if !self.dropped_reported {
                        self.dropped_reported = true;
                        self.inbound.push(ConnEvent::Dropped);
                    }
                    return;
                }
            }
        }

        let mut read_pos = 0;
        while read_pos < self.recv_buf.len() {
            if self.inbound.len() >= MAX_PACKETS_PER_POLL {
                break;
            }
            match try_decode_packet_slice(&self.recv_buf[read_pos..]) {
                Ok(Some((PacketData::KickDisconnect { .. }, consumed))) => {
                    read_pos += consumed;
                    self.closed = true;
                    if !self.dropped_reported {
                        self.dropped_reported = true;
                        self.inbound.push(ConnEvent::Dropped);
                    }
                    break;
                }
                Ok(Some((pkt, consumed))) => {
                    read_pos += consumed;
                    self.inbound.push(ConnEvent::Packet(pkt));
                }
                Ok(None) => break,
                Err(_) => {
                    self.closed = true;
                    if !self.dropped_reported {
                        self.dropped_reported = true;
                        self.inbound.push(ConnEvent::Dropped);
                    }
                    break;
                }
            }
        }
        if read_pos > 0 {
            if read_pos >= self.recv_buf.len() {
                self.recv_buf.clear();
            } else {
                self.recv_buf.drain(..read_pos);
            }
        }
        if self.recv_buf.is_empty() && self.recv_buf.capacity() > 524288 {
            self.recv_buf.shrink_to(65536);
        }
    }

    fn flush_outbound(&mut self) {
        if self.closed {
            return;
        }
        let mut write_pos = 0;
        while write_pos < self.outbound.len() {
            match self.stream.write(&self.outbound[write_pos..]) {
                Ok(0) => {
                    self.closed = true;
                    break;
                }
                Ok(n) => {
                    write_pos += n;
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    break;
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => {
                    continue;
                }
                Err(_) => {
                    self.closed = true;
                    break;
                }
            }
        }
        if write_pos > 0 {
            if write_pos >= self.outbound.len() {
                self.outbound.clear();
            } else {
                self.outbound.drain(..write_pos);
            }
        }
        if self.outbound.is_empty() && self.outbound.capacity() > 524288 {
            self.outbound.shrink_to(65536);
        }
        if self.outbound.is_empty() && self.closing && !self.closed {
            let _ = self.stream.shutdown(std::net::Shutdown::Write);
            self.closed = true;
        }
    }
}

/// Owned TCP connection: event-driven non-blocking I/O via `mio`.
pub struct Conn {
    inner: RefCell<ConnInner>,
    pub remote: String,
}

impl Conn {
    pub fn new(stream: std::net::TcpStream) -> std::io::Result<Self> {
        // No peer address means we cannot rate-limit or ban by IP:
        // drop the connection outright instead of lumping it into the
        // empty-IP bucket.
        let remote = stream
            .peer_addr()
            .map(|a| a.to_string())
            .map_err(|e| e)?;
        let _ = stream.set_nodelay(true);
        stream.set_nonblocking(true)?;
        let stream = TcpStream::from_std(stream);
        Ok(Self::from_mio(stream, remote))
    }

    pub fn from_mio(stream: TcpStream, remote: String) -> Self {
        let _ = stream.set_nodelay(true);
        Self {
            inner: RefCell::new(ConnInner {
                stream,
                recv_buf: Vec::with_capacity(1024),
                outbound: Vec::with_capacity(1024),
                inbound: Vec::new(),
                recv_limit: RECV_QUEUE_LOGIN_MAX_BYTES,
                closed: false,
                closing: false,
                dropped_reported: false,
            }),
            remote,
        }
    }

    /// Raise the receive cap (login leg -> authenticated play).
    pub fn set_recv_limit(&self, limit: usize) {
        self.inner.borrow_mut().recv_limit = limit;
    }

    /// Socket fully closed (nothing more will be written or read)?
    pub fn is_closed(&self) -> bool {
        self.inner.borrow().closed
    }

    /// Read available bytes and decode packets.
    pub fn poll_read(&self) {
        self.inner.borrow_mut().poll_read();
    }

    /// Flush queued outbound bytes to the socket.
    pub fn flush_outbound(&self) {
        self.inner.borrow_mut().flush_outbound();
    }

    /// Non-blocking drain of queued events.
    pub fn drain(&self) -> Vec<ConnEvent> {
        let mut inner = self.inner.borrow_mut();
        inner.poll_read();
        let mut out = std::mem::take(&mut inner.inbound);
        if inner.closed && !inner.dropped_reported {
            inner.dropped_reported = true;
            out.push(ConnEvent::Dropped);
        }
        out
    }

    /// Queue bytes for the socket (bounds queue size and flushes).
    pub fn send(&self, bytes: Vec<u8>) {
        let mut inner = self.inner.borrow_mut();
        if inner.closed {
            return;
        }
        let len = bytes.len();
        if inner.outbound.len() + len > SEND_QUEUE_MAX_BYTES as usize {
            crate::server_log::warning(&format!(
                "Send queue limit exceeded ({} bytes) for {}, dropping connection",
                inner.outbound.len() + len,
                self.remote
            ));
            drop(inner);
            self.close();
            return;
        }
        inner.outbound.extend_from_slice(&bytes);
        inner.flush_outbound();
    }

    /// Queue multiple packets for the socket in a single batch, flushing once.
    pub fn send_batch(&self, packets: &mut Vec<Vec<u8>>) {
        let mut inner = self.inner.borrow_mut();
        if inner.closed {
            packets.clear();
            return;
        }
        let total_len: usize = packets.iter().map(|p| p.len()).sum();
        if inner.outbound.len() + total_len > SEND_QUEUE_MAX_BYTES as usize {
            crate::server_log::warning(&format!(
                "Send queue limit exceeded ({} bytes) for {}, dropping connection",
                inner.outbound.len() + total_len,
                self.remote
            ));
            drop(inner);
            self.close();
            packets.clear();
            return;
        }
        for pkt in packets.drain(..) {
            inner.outbound.extend_from_slice(&pkt);
        }
        inner.flush_outbound();
    }

    /// Queue bytes for the socket without flushing immediately.
    pub fn queue(&self, bytes: &[u8]) {
        let mut inner = self.inner.borrow_mut();
        if inner.closed {
            return;
        }
        let len = bytes.len();
        if inner.outbound.len() + len > SEND_QUEUE_MAX_BYTES as usize {
            crate::server_log::warning(&format!(
                "Send queue limit exceeded ({} bytes) for {}, dropping connection",
                inner.outbound.len() + len,
                self.remote
            ));
            drop(inner);
            self.close();
            return;
        }
        inner.outbound.extend_from_slice(bytes);
    }

    /// Graceful close: flush outbound bytes, FIN the socket, and report Dropped.
    pub fn close(&self) {
        let mut inner = self.inner.borrow_mut();
        inner.closing = true;
        inner.flush_outbound();
        if inner.outbound.is_empty() {
            let _ = inner.stream.shutdown(std::net::Shutdown::Both);
            inner.closed = true;
            if !inner.dropped_reported {
                inner.dropped_reported = true;
                inner.inbound.push(ConnEvent::Dropped);
            }
        }
    }

    /// Crate-internal mutable access to underlying mio TcpStream for poll registration.
    pub(crate) fn with_stream_mut<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut TcpStream) -> R,
    {
        let mut inner = self.inner.borrow_mut();
        f(&mut inner.stream)
    }

    /// Has any outbound bytes pending to be flushed?
    pub(crate) fn has_pending_outbound(&self) -> bool {
        !self.inner.borrow().outbound.is_empty()
    }
}

/// Bind a listener for the accept loop (the server slice drives it).
pub fn bind_listener(addr: &str) -> std::io::Result<TcpListener> {
    TcpListener::bind(addr)
}

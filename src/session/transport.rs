//! Socket transport: non-blocking `mio` TCP connection and packet framing.

use std::cell::RefCell;
use std::io::{Read, Write};
use std::net::TcpListener;
use mio::net::TcpStream;
use crate::network::{PacketData, try_decode_packet};
use crate::server_constants::SEND_QUEUE_MAX_BYTES;

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
                    if self.recv_buf.len() > SEND_QUEUE_MAX_BYTES as usize {
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

        while !self.recv_buf.is_empty() {
            match try_decode_packet(&mut self.recv_buf) {
                Ok(Some(PacketData::KickDisconnect { .. })) => {
                    self.closed = true;
                    if !self.dropped_reported {
                        self.dropped_reported = true;
                        self.inbound.push(ConnEvent::Dropped);
                    }
                    break;
                }
                Ok(Some(pkt)) => {
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
    }

    fn flush_outbound(&mut self) {
        if self.closed {
            return;
        }
        while !self.outbound.is_empty() {
            match self.stream.write(&self.outbound) {
                Ok(0) => {
                    self.closed = true;
                    return;
                }
                Ok(n) => {
                    self.outbound.drain(..n);
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    break;
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => {
                    continue;
                }
                Err(_) => {
                    self.closed = true;
                    return;
                }
            }
        }
        if self.outbound.is_empty() && self.closing {
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
        let remote = stream.peer_addr().map(|a| a.to_string()).unwrap_or_default();
        stream.set_nonblocking(true)?;
        let stream = TcpStream::from_std(stream);
        Ok(Self::from_mio(stream, remote))
    }

    pub fn from_mio(stream: TcpStream, remote: String) -> Self {
        Self {
            inner: RefCell::new(ConnInner {
                stream,
                recv_buf: Vec::with_capacity(1024),
                outbound: Vec::with_capacity(1024),
                inbound: Vec::new(),
                closed: false,
                closing: false,
                dropped_reported: false,
            }),
            remote,
        }
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

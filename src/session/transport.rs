//! Socket transport: `Conn` read/write threads and packet framing.
//! Split out of `session.rs`; behavior unchanged.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use crate::network::{PacketData, read_packet_payload};
use crate::server_constants::SEND_QUEUE_MAX_BYTES;

// ---- connection pump (one read + one write thread per socket) ----

/// Connection event: a decoded packet or a dead socket.
pub enum ConnEvent {
    Packet(PacketData),
    Dropped,
}

/// Owned TCP connection: a read thread decodes packets into `inbound`, a
/// write thread ships `outbound` byte blobs. Either thread exiting means
/// the socket is gone (mirrors the C++ read/write thread pair).
pub struct Conn {
    inbound: Receiver<ConnEvent>,
    outbound: Sender<Vec<u8>>,
    closer: Sender<()>,
    release: Option<TcpStream>,
    queued_bytes: Arc<AtomicUsize>,
    pub remote: String,
}

impl Conn {
    pub fn new(stream: TcpStream) -> std::io::Result<Self> {
        let remote = stream.peer_addr().map(|a| a.to_string()).unwrap_or_default();
        // Blocking reads like the C++ network manager: idle policy lives
        // one level up (login 600 / play 1200 ticks with dead bypass), so
        // the socket layer must not pre-empt it (death screens go quiet).
        stream.set_read_timeout(None)?;
        let release = stream.try_clone().ok();
        let mut reader = stream.try_clone()?;
        let mut writer = stream;
        let (tx_in, inbound) = mpsc::channel();
        let (outbound, rx_out): (Sender<Vec<u8>>, Receiver<Vec<u8>>) = mpsc::channel();
        let (closer, close_rx): (Sender<()>, Receiver<()>) = mpsc::channel();
        let queued_bytes = Arc::new(AtomicUsize::new(0));
        let qb_writer = Arc::clone(&queued_bytes);

        thread::Builder::new()
            .name(format!("conn-read-{remote}"))
            .spawn(move || {
                loop {
                    let mut id = [0u8; 1];
                    if reader.read_exact(&mut id).is_err() {
                        break;
                    }
                    match read_packet_payload(&mut reader, id[0]) {
                        Ok(PacketData::KickDisconnect { .. }) => break,
                        Ok(pkt) => {
                            if tx_in.send(ConnEvent::Packet(pkt)).is_err() {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
                let _ = tx_in.send(ConnEvent::Dropped);
            })
            .map_err(std::io::Error::other)?;

        thread::Builder::new()
            .name(format!("conn-write-{remote}"))
            .spawn(move || {
                // Drain-then-FIN: queued kick bytes must reach the client,
                // so close only stops the writer after the queue empties
                // (a Both-shutdown here would RST pending data away).
                loop {
                    while let Ok(msg) = rx_out.try_recv() {
                        qb_writer.fetch_sub(msg.len(), Ordering::Relaxed);
                        if writer.write_all(&msg).is_err() || writer.flush().is_err() {
                            return;
                        }
                    }
                    if close_rx.try_recv().is_ok() {
                        let _ = writer.flush();
                        let _ = writer.shutdown(std::net::Shutdown::Write);
                        return;
                    }
                    match rx_out.recv_timeout(Duration::from_millis(20)) {
                        Ok(msg) => {
                            qb_writer.fetch_sub(msg.len(), Ordering::Relaxed);
                            if writer.write_all(&msg).is_err() || writer.flush().is_err() {
                                return;
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) => return,
                    }
                }
            })
            .map_err(std::io::Error::other)?;

        Ok(Self { inbound, outbound, closer, release, queued_bytes, remote })
    }

    /// Non-blocking drain of queued events.
    pub fn drain(&self) -> Vec<ConnEvent> {
        let mut out = Vec::new();
        while let Ok(ev) = self.inbound.try_recv() {
            out.push(ev);
        }
        out
    }

    /// Queue bytes for the socket (drops silently once dead, bounds queue size).
    pub fn send(&self, bytes: Vec<u8>) {
        let len = bytes.len();
        let prev = self.queued_bytes.fetch_add(len, Ordering::Relaxed);
        if prev + len > SEND_QUEUE_MAX_BYTES as usize {
            self.queued_bytes.fetch_sub(len, Ordering::Relaxed);
            crate::server_log::warning(&format!(
                "Send queue limit exceeded ({} bytes) for {}, dropping connection",
                prev + len,
                self.remote
            ));
            self.close();
            return;
        }
        let _ = self.outbound.send(bytes);
    }

    /// Graceful close: the writer drains queued bytes and FINs (the
    /// server calls this on kick/timeout/shutdown); the blocked reader
    /// is released separately so ghost threads cannot linger.
    pub fn close(&self) {
        let _ = self.closer.send(());
        if let Some(s) = &self.release {
            let _ = s.shutdown(std::net::Shutdown::Read);
        }
    }
}

/// Bind a listener for the accept loop (the server slice drives it).
pub fn bind_listener(addr: &str) -> std::io::Result<TcpListener> {
    TcpListener::bind(addr)
}

//! Login handshake state machine (mirrors `NetLoginHandler`).
//! Split out of `session.rs`; behavior unchanged.

use std::sync::mpsc::{self, Receiver};
use std::thread;
use crate::network::PacketData;
use crate::session_packets::{pkt_handshake, pkt_kick};

// ---- login state machine (mirrors NetLoginHandler) ----

use std::sync::atomic::{AtomicUsize, Ordering};

pub const DEFAULT_AUTH_URL: &str =
    "http://session.betacraft.uk/game/checkserver.jsp?user={user}&serverId={serverId}";

static ACTIVE_VERIFICATIONS: AtomicUsize = AtomicUsize::new(0);
const MAX_CONCURRENT_VERIFICATIONS: usize = 8;

/// Session verification: the default hits the BetaCraft / custom check endpoint
/// (blocking HTTP, run on a bounded worker thread); tests inject a stub.
pub type VerifyFn = Box<dyn Fn(&str, &str) -> Result<String, String> + Send>;

/// Creates a verifier for a specific URL template (e.g. BetaCraft proxy).
pub fn make_default_verify(url_template: String) -> VerifyFn {
    Box::new(move |username: &str, server_id: &str| {
        let url = url_template
            .replace("{user}", &urlencoding(username))
            .replace("{serverId}", &urlencoding(server_id));
        // ureq 3: timeouts live on the Agent config (no per-request
        // `.timeout()` anymore); 5s global preserves the ureq 2 behavior.
        // No built-in retry in v3 (v2 retried GET silently) — one attempt,
        // fail-closed like before.
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(std::time::Duration::from_secs(5)))
            .build();
        let agent: ureq::Agent = config.into();
        let mut resp = agent.get(&url).call().map_err(|e| e.to_string())?;
        let body = resp
            .body_mut()
            .read_to_string()
            .map_err(|e| e.to_string())?;
        let reply = body.trim().to_string();
        if reply == "YES" {
            Ok(reply)
        } else {
            Err("Session verification failed".to_string())
        }
    })
}

/// Default verifier using BetaCraft legacy authentication proxy.
pub fn default_verify(username: &str, server_id: &str) -> Result<String, String> {
    make_default_verify(DEFAULT_AUTH_URL.to_string())(username, server_id)
}

fn urlencoding(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(byte as char);
            }
            _ => {
                result.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    result
}

/// Terminal login outcome for the server tick.
pub enum LoginEvent {
    /// Hand the username to player creation (packets already queued).
    Accepted { username: String },
    /// Kicked or gone; kick bytes (if any) are already queued.
    Done,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LoginState {
    WaitHandshake,
    WaitLogin,
    Verifying,
    Finished,
}

/// Login session: feeds inbound packets, queues outbound bytes, and
/// reports its outcome through `poll` (mirrors `tryLogin`).
pub struct LoginSession {
    online_mode: bool,
    server_id: String,
    username: String,
    ticks: u32,
    state: LoginState,
    verifying: bool,
    verify_rx: Option<Receiver<Result<String, String>>>,
    verify: Option<VerifyFn>,
    finished: bool,
    done_sent: bool,
    /// Bytes for the socket, in order.
    pub outbox: Vec<Vec<u8>>,
}

impl LoginSession {
    pub fn new(online_mode: bool) -> Self {
        Self {
            online_mode,
            server_id: String::new(),
            username: String::new(),
            ticks: 0,
            state: LoginState::WaitHandshake,
            verifying: false,
            verify_rx: None,
            verify: Some(Box::new(default_verify)),
            finished: false,
            done_sent: false,
            outbox: Vec::new(),
        }
    }

    /// Test hook: replace the HTTP verifier.
    /// Test hook: replace the HTTP verifier.
    pub fn with_verify(mut self, f: VerifyFn) -> Self {
        self.verify = Some(f);
        self
    }

    /// Set the authentication URL template.
    pub fn with_auth_url(mut self, url: String) -> Self {
        self.verify = Some(make_default_verify(url));
        self
    }

    fn kick(&mut self, reason: &str) {
        if self.finished {
            return;
        }
        self.outbox.push(pkt_kick(reason));
        self.finished = true;
        self.state = LoginState::Finished;
    }

    /// Feed one inbound packet (unexpected packets are ignored like the
    /// C++ handler set, which only overrides handshake/login/error).
    pub fn on_packet(&mut self, pkt: PacketData) {
        if self.finished {
            return;
        }
        match pkt {
            PacketData::Handshake { username: _ } => {
                if self.online_mode {
                    // Nonce like C++ (hex of a uniform i64).
                    let rand_val = nonce_i64();
                    self.server_id = format!("{:x}", rand_val as u64);
                    self.outbox.push(pkt_handshake(&self.server_id.clone()));
                } else {
                    self.outbox.push(pkt_handshake("-"));
                }
                if self.state == LoginState::WaitHandshake {
                    self.state = LoginState::WaitLogin;
                }
            }
            PacketData::Login { protocol_version, mut username, .. } => {
                while username.ends_with(['\0', '\r', '\n', ' ']) {
                    username.pop();
                }
                if username.is_empty() || username.len() > 16 {
                    self.kick("Invalid username length");
                    return;
                }
                if !username.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_') {
                    self.kick("Invalid username characters");
                    return;
                }
                self.username = username.clone();
                if protocol_version != 6 {
                    if protocol_version > 6 {
                        self.kick("Outdated server!");
                    } else {
                        self.kick("Outdated client!");
                    }
                    return;
                }
                if !self.online_mode {
                    self.finished = true;
                    return;
                }
                if self.verifying {
                    self.kick("Duplicate login packet");
                    return;
                }
                if ACTIVE_VERIFICATIONS.load(Ordering::Relaxed) >= MAX_CONCURRENT_VERIFICATIONS {
                    self.kick("Too many login attempts, please try again shortly");
                    return;
                }
                ACTIVE_VERIFICATIONS.fetch_add(1, Ordering::SeqCst);
                self.verifying = true;
                self.state = LoginState::Verifying;
                let (tx, rx) = mpsc::channel();
                self.verify_rx = Some(rx);
                let verify = self.verify.take();
                let sid = self.server_id.clone();
                let uname = username.clone();
                if thread::Builder::new()
                    .name(format!("login-verify-{uname}"))
                    .spawn(move || {
                        struct VerifyGuard;
                        impl Drop for VerifyGuard {
                            fn drop(&mut self) {
                                ACTIVE_VERIFICATIONS.fetch_sub(1, Ordering::SeqCst);
                            }
                        }
                        let _guard = VerifyGuard;
                        let res = match verify {
                            Some(f) => f(&uname, &sid),
                            None => Err("no verifier".to_string()),
                        };
                        let _ = tx.send(res);
                    })
                    .is_err()
                {
                    // Thread spawn failed (EMFILE/OOM): release the slot or
                    // the counter leaks and all future logins get rejected.
                    ACTIVE_VERIFICATIONS.fetch_sub(1, Ordering::SeqCst);
                    self.verifying = false;
                    self.verify_rx = None;
                    self.kick("Login service unavailable, try again shortly");
                }
            }
            _ => {}
        }
    }

    /// Remote hung up (mirrors `handleErrorMessage`).
    pub fn on_drop(&mut self) {
        self.finished = true;
        self.state = LoginState::Finished;
    }

    /// Tick the timeout and the verifier result (mirrors `tryLogin`).
    pub fn poll(&mut self) -> Option<LoginEvent> {
        if self.done_sent {
            return None;
        }
        if self.state == LoginState::Verifying {
            if let Some(rx) = &self.verify_rx {
                match rx.try_recv() {
                    Ok(Ok(reply)) => {
                        if reply == "YES" {
                            self.finished = true;
                            self.done_sent = true;
                            return Some(LoginEvent::Accepted {
                                username: std::mem::take(&mut self.username),
                            });
                        }
                        self.kick("Failed to verify username!");
                        self.done_sent = true;
                        return Some(LoginEvent::Done);
                    }
                    Ok(Err(_)) => {
                        self.kick("Failed to verify username!");
                        self.done_sent = true;
                        return Some(LoginEvent::Done);
                    }
                    Err(_) => {}
                }
            }
        }
        // Offline accept lands here (finished, still WaitLogin); kicks
        // and drops land in Finished.
        if self.finished {
            self.done_sent = true;
            if self.state == LoginState::Finished {
                return Some(LoginEvent::Done);
            }
            return Some(LoginEvent::Accepted { username: std::mem::take(&mut self.username) });
        }
        self.ticks += 1;
        if self.ticks >= 600 {
            self.kick("Took too long to log in");
            self.done_sent = true;
            return Some(LoginEvent::Done);
        }
        None
    }
}

/// Cheap nonce (server ids need uniqueness, not determinism).
fn nonce_i64() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E3779B97F4A7C15);
    let mut x = nanos | 1;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    x as i64
}

#[cfg(test)]
#[path = "login_tests.rs"]
mod login_tests;

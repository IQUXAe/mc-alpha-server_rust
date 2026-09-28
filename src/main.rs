//! Server binary: properties, world open, socket bind, console thread, 20
//! TPS tick loop with lag accounting, graceful shutdown on stop/signal.
//!
//! Usage: `alpha_server [server.properties] [world-dir] [port]`
//! (all optional; the world defaults to `world/<level-name>`, the port
//! to `server-port`).

use std::io::BufRead;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use alpha_server::server::Server;
use alpha_server::server_config::ServerConfig;
use alpha_server::server_log as log;

static STOP: AtomicBool = AtomicBool::new(false);

#[allow(unsafe_code)]
extern "C" fn signal_handler(signum: libc::c_int) {
    if STOP.swap(true, Ordering::SeqCst) {
        // Second signal: exit immediately using async-signal-safe _exit.
        unsafe {
            libc::_exit(128 + signum);
        }
    }
}

#[allow(unsafe_code)]
fn main() {
    println!("  ___  _      _         ___                      ");
    println!(" / _ \\| |_ __| |_  __ _/ __| ___ _ ___ _____ _ _ ");
    println!("| (_) | | '_ \\ ' \\/ _` \\__ \\/ -_) '_\\ V / -_) '_|");
    println!(" \\___/|_| .__/_||_\\__,_|___/\\___|_|  \\_/\\___|_|  ");
    println!("        |_|  Minecraft Alpha 1.2.6 - Rust Edition ");
    println!();

    let args: Vec<String> = std::env::args().collect();
    let props_path = args.get(1).map(String::as_str).unwrap_or("server.properties");

    log::info("Starting Minecraft server version 0.2.8 (Alpha 1.2.6)");
    log::info("Loading properties");
    let level_name = ServerConfig::open(props_path).get_string("level-name", "world");
    let world_dir = args
        .get(2)
        .cloned()
        .unwrap_or_else(|| format!("world/{level_name}"));
    let player_dir = format!("{world_dir}/players");

    let mut server = match Server::open(
        props_path,
        &world_dir,
        &player_dir,
        "ops.txt",
        "banned-players.txt",
        "banned-ips.txt",
    ) {
        Ok(s) => s,
        Err(e) => {
            log::severe(&format!("Failed to initialize server: {e}"));
            std::process::exit(1);
        }
    };
    if let Some(port_arg) = args.get(3) {
        match port_arg.parse::<i32>() {
            Ok(port) => server.settings.port = port,
            Err(_) => log::warning(&format!(
                "Invalid port argument '{port_arg}', using {}",
                server.settings.port
            )),
        }
    }
    if !server.settings.online_mode {
        log::warning("**** SERVER IS RUNNING IN OFFLINE/INSECURE MODE!");
        log::warning("The server will make no attempt to authenticate usernames. Beware.");
        log::warning("While this makes the game possible to play without internet access, it also opens up the ability for hackers to connect with any username they choose.");
        log::warning("To change this, set \"online-mode\" to \"true\" in the server.properties file.");
    }

    let bind_ip = server.settings.server_ip.clone();
    let port = server.settings.port;
    let addr = format!("{}:{port}", if bind_ip.is_empty() { "0.0.0.0" } else { &bind_ip });
    if let Err(e) = server.bind_listener(&addr) {
        log::warning("**** FAILED TO BIND TO PORT!");
        log::warning(&format!("The exception was: {e}"));
        log::warning("Perhaps a server is already running on that port?");
        std::process::exit(1);
    }
    let display = if bind_ip.is_empty() { "*" } else { bind_ip.as_str() };
    log::info(&format!("Starting Minecraft server on {display}:{port}"));
    log::info("Done! For help, type \"help\" or \"?\"");

    // SAFETY: `signal_handler` is a capture-free `extern "C"` fn, so its
    // address is a valid `sighandler_t` for `libc::signal` to store.
    unsafe {
        let handler = signal_handler as *const () as libc::sighandler_t;
        libc::signal(libc::SIGINT, handler);
        libc::signal(libc::SIGTERM, handler);
    }

    let (console_tx, console_rx) = std::sync::mpsc::channel::<String>();
    let waker = std::sync::Arc::clone(&server.waker);
    std::thread::Builder::new()
        .name("console".to_string())
        .spawn(move || {
            let stdin = std::io::stdin();
            for line in stdin.lock().lines() {
                match line {
                    Ok(l) => {
                        if console_tx.send(l).is_err() {
                            break;
                        }
                        let _ = waker.wake();
                    }
                    Err(_) => break,
                }
            }
        })
        .ok();

    // Fixed-timestep tick loop (20 TPS = 50ms ticks).
    let tick_duration = Duration::from_millis(50);
    let mut next_tick = Instant::now();
    loop {
        while let Ok(line) = console_rx.try_recv() {
            server.queue_console(line);
        }
        if STOP.load(Ordering::SeqCst) {
            server.shutdown();
            break;
        }

        let now = Instant::now();
        if now >= next_tick {
            let behind = now.duration_since(next_tick);
            if behind > Duration::from_secs(2) {
                log::warning(
                    "Can't keep up! Did the system time change, or is the server overloaded?",
                );
                next_tick = now;
            }
            while next_tick <= now {
                next_tick += tick_duration;
                server.tick();
                if !server.running {
                    server.shutdown();
                    break;
                }
            }
            if !server.running {
                break;
            }
        }

        let now = Instant::now();
        let poll_timeout = next_tick.saturating_duration_since(now);
        server.poll_network(poll_timeout);
    }
    log::info("Waiting for background threads to finish saving...");
}

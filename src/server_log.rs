//! Simple timestamped logger.
//!
//! Format: `YYYY-MM-DD HH:MM:SS.mmm [PREFIX] message` with prefixes `[D]`,
//! `[INFO]`, `[WARNING]` and `[SEVERE]`. Messages below the static minimum
//! level (default `INFO`) are dropped, and warnings/errors go to stderr
//! while the rest goes to stdout.
//!
//! Logging is synchronous, and the console-line provider is an optional boxed
//! closure re-printed after each line.

use std::io::{IsTerminal, Write};
use std::sync::{
    Mutex, OnceLock,
    atomic::{AtomicU8, Ordering},
};

/// Severity levels, ordered `DEBUG < INFO < WARNING < SEVERE`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum LogLevel {
    Debug = 0,
    Info = 1,
    Warning = 2,
    Severe = 3,
}

impl LogLevel {
    /// Prefix used in the formatted line.
    pub const fn prefix(self) -> &'static str {
        match self {
            LogLevel::Debug => "[D]",
            LogLevel::Info => "[INFO]",
            LogLevel::Warning => "[WARNING]",
            LogLevel::Severe => "[SEVERE]",
        }
    }

    fn from_u8(value: u8) -> Self {
        match value {
            0 => LogLevel::Debug,
            2 => LogLevel::Warning,
            3 => LogLevel::Severe,
            _ => LogLevel::Info,
        }
    }
}

static MIN_LEVEL: AtomicU8 = AtomicU8::new(LogLevel::Info as u8);

/// Sets the minimum level; messages below it are dropped.
pub fn set_level(level: LogLevel) {
    MIN_LEVEL.store(level as u8, Ordering::Relaxed);
}

/// Returns the current minimum level.
pub fn get_level() -> LogLevel {
    LogLevel::from_u8(MIN_LEVEL.load(Ordering::Relaxed))
}

/// True when `level` passes the current minimum-level filter.
pub fn should_log(level: LogLevel) -> bool {
    get_level() <= level
}

type ConsoleLineProvider = Box<dyn Fn() -> String + Send + Sync>;

fn provider_slot() -> &'static Mutex<Option<ConsoleLineProvider>> {
    static SLOT: OnceLock<Mutex<Option<ConsoleLineProvider>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

fn lock_slot() -> std::sync::MutexGuard<'static, Option<ConsoleLineProvider>> {
    match provider_slot().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Installs the console-line provider re-printed after each line.
pub fn set_console_line_provider(provider: ConsoleLineProvider) {
    *lock_slot() = Some(provider);
}

/// Removes the console-line provider, if any.
pub fn clear_console_line_provider() {
    *lock_slot() = None;
}

fn current_console_line() -> Option<String> {
    let slot = lock_slot();
    slot.as_ref().map(|provider| provider())
}

fn stdout_is_tty() -> bool {
    std::io::stdout().is_terminal()
}

fn clear_console_line() {
    if stdout_is_tty() {
        print!("\r\x1b[2K\r");
    }
}

fn print_console_line() {
    if let Some(line) = current_console_line() {
        clear_console_line();
        print!("{line}");
        let _ = std::io::stdout().flush();
    }
}

/// UTC timestamp in `%Y-%m-%d %H:%M:%S.mmm` layout (pure std, no libc).
fn current_timestamp() -> String {
    let now = std::time::SystemTime::now();
    let since_epoch = match now.duration_since(std::time::UNIX_EPOCH) {
        Ok(elapsed) => elapsed,
        Err(_) => std::time::Duration::ZERO,
    };
    let secs = since_epoch.as_secs() as i64;
    let millis = since_epoch.subsec_millis();
    let (y, mo, d, h, mi, s) = unix_to_ymd_hms(secs);
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}.{millis:03} ")
}

/// Convert unix seconds to (y, mo, d, h, mi, s) in UTC.
/// Days part via Howard Hinnant's civil_from_days; time part by division.
fn unix_to_ymd_hms(secs: i64) -> (i32, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86400);
    let secs_of_day = secs.rem_euclid(86400) as u32;
    // civil_from_days, Howard Hinnant algorithm.
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (
        y as i32,
        m as u32,
        d as u32,
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60,
    )
}

/// Formats one line: timestamp, prefix and message.
pub fn format_message(level: LogLevel, msg: &str) -> String {
    format!("{}{} {}", current_timestamp(), level.prefix(), msg)
}

enum LogCommand {
    Line(LogLevel, String),
    PrintConsole,
    Flush(std::sync::mpsc::SyncSender<()>),
}

fn log_channel() -> &'static std::sync::mpsc::SyncSender<LogCommand> {
    static SENDER: OnceLock<std::sync::mpsc::SyncSender<LogCommand>> = OnceLock::new();
    SENDER.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::sync_channel::<LogCommand>(4096);
        std::thread::Builder::new()
            .name("server-logger".to_string())
            .spawn(move || {
                while let Ok(cmd) = rx.recv() {
                    match cmd {
                        LogCommand::Line(level, line) => {
                            match level {
                                LogLevel::Warning | LogLevel::Severe => eprintln!("{line}"),
                                LogLevel::Debug | LogLevel::Info => println!("{line}"),
                            }
                            print_console_line();
                        }
                        LogCommand::PrintConsole => {
                            print_console_line();
                        }
                        LogCommand::Flush(ack) => {
                            let _ = std::io::stdout().flush();
                            let _ = std::io::stderr().flush();
                            let _ = ack.send(());
                        }
                    }
                }
            })
            .expect("failed to spawn server-logger thread");
        tx
    })
}

/// Logs one line if it passes the filter (stderr for warnings/errors),
/// non-blocking via a background thread to prevent tick thread stalls.
pub fn log(level: LogLevel, msg: &str) {
    if !should_log(level) {
        return;
    }
    let line = format_message(level, msg);
    let _ = log_channel().try_send(LogCommand::Line(level, line));
}

/// Logs at DEBUG.
pub fn debug(msg: &str) {
    log(LogLevel::Debug, msg);
}

/// Logs at INFO.
pub fn info(msg: &str) {
    log(LogLevel::Info, msg);
}

/// Logs at WARNING.
pub fn warning(msg: &str) {
    log(LogLevel::Warning, msg);
}

/// Logs at SEVERE.
pub fn severe(msg: &str) {
    log(LogLevel::Severe, msg);
}

/// Re-prints the console line via the logger queue.
pub fn refresh_console_line() {
    let _ = log_channel().try_send(LogCommand::PrintConsole);
}

/// Flushes pending log output from the background worker.
pub fn flush() {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    if log_channel().send(LogCommand::Flush(tx)).is_ok() {
        let _ = rx.recv();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_are_ordered() {
        assert!(LogLevel::Debug < LogLevel::Info);
        assert!(LogLevel::Info < LogLevel::Warning);
        assert!(LogLevel::Warning < LogLevel::Severe);
    }

    #[test]
    fn prefixes_match_format_message() {
        assert_eq!(LogLevel::Debug.prefix(), "[D]");
        assert_eq!(LogLevel::Info.prefix(), "[INFO]");
        assert_eq!(LogLevel::Warning.prefix(), "[WARNING]");
        assert_eq!(LogLevel::Severe.prefix(), "[SEVERE]");
    }

    #[test]
    fn formatted_line_carries_prefix_and_message() {
        for level in [
            LogLevel::Debug,
            LogLevel::Info,
            LogLevel::Warning,
            LogLevel::Severe,
        ] {
            let line = format_message(level, "hello");
            assert!(line.contains(level.prefix()));
            assert!(line.ends_with("hello"));
            // Timestamp shape: "YYYY-MM-DD HH:MM:SS.mmm ...".
            assert_eq!(line.as_bytes().get(4), Some(&b'-'));
            assert_eq!(line.as_bytes().get(10), Some(&b' '));
            assert_eq!(line.as_bytes().get(19), Some(&b'.'));
        }
    }

    #[test]
    fn unknown_stored_level_falls_back_to_info() {
        assert_eq!(LogLevel::from_u8(0), LogLevel::Debug);
        assert_eq!(LogLevel::from_u8(1), LogLevel::Info);
        assert_eq!(LogLevel::from_u8(2), LogLevel::Warning);
        assert_eq!(LogLevel::from_u8(3), LogLevel::Severe);
        assert_eq!(LogLevel::from_u8(99), LogLevel::Info);
    }

    // Single global-mutating test so parallel test threads cannot race on
    // the shared minimum level / console-line provider.
    #[test]
    fn level_filtering_and_console_provider() {
        set_level(LogLevel::Info);
        assert_eq!(get_level(), LogLevel::Info);
        assert!(!should_log(LogLevel::Debug));
        assert!(should_log(LogLevel::Info));
        assert!(should_log(LogLevel::Warning));
        assert!(should_log(LogLevel::Severe));

        set_level(LogLevel::Warning);
        assert!(!should_log(LogLevel::Info));
        assert!(should_log(LogLevel::Warning));
        assert!(should_log(LogLevel::Severe));

        set_level(LogLevel::Debug);
        assert!(should_log(LogLevel::Debug));

        set_level(LogLevel::Severe);
        assert!(!should_log(LogLevel::Warning));
        assert!(should_log(LogLevel::Severe));

        // Filtered-out lines are dropped without touching the provider.
        set_console_line_provider(Box::new(|| "> ".to_string()));
        log(LogLevel::Debug, "dropped");
        // Visible lines and explicit refresh re-print the provider line.
        log(LogLevel::Severe, "shown");
        refresh_console_line();
        clear_console_line_provider();
        refresh_console_line();
        flush();

        set_level(LogLevel::Info);
        assert_eq!(get_level(), LogLevel::Info);
    }

    #[test]
    fn level_helpers_do_not_panic() {
        set_level(LogLevel::Debug);
        debug("d");
        info("i");
        warning("w");
        severe("s");
        set_level(LogLevel::Info);
    }
}

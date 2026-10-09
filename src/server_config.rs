//! Server properties (mirrors Java `PropertyManager`).
//!
//! Format: `key=value` lines, `#` comment lines and empty
//! lines skipped, lines without `=` ignored. Keys lose trailing `' '` and
//! values lose leading `' '` (spaces only). The first occurrence of a key
//! wins. `save()` writes the `#Minecraft server properties` header followed
//! by `key=value` lines.
//!
//! This is the only config module allowed to touch the filesystem: missing
//! keys are persisted back via `save()`, and opening a missing file prints
//! the same warning/generation notices before creating it.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Header written at the top of every saved properties file.
pub const PROPERTIES_HEADER: &str = "#Minecraft server properties";

/// `server.properties` key/value store with C++-compatible load/save.
#[derive(Clone, Debug, Default)]
pub struct ServerConfig {
    path: PathBuf,
    props: HashMap<String, String>,
}

impl ServerConfig {
    /// Mirrors the C++ constructor: loads the file when it exists, otherwise
    /// reports it and generates a fresh (header-only) file.
    pub fn open(path: impl AsRef<Path>) -> Self {
        let mut config = Self {
            path: path.as_ref().to_path_buf(),
            props: HashMap::new(),
        };
        if config.path.exists() {
            config.load();
        } else {
            eprintln!("[WARNING] {} does not exist", config.path.display());
            println!("[INFO] Generating new properties file");
            config.save();
        }
        config
    }

    /// Construct in-memory ServerConfig from parsed text (useful for tests).
    pub fn from_text(text: &str) -> Self {
        Self {
            path: PathBuf::new(),
            props: Self::parse_text(text),
        }
    }

    /// Construct in-memory ServerConfig from a HashMap (useful for tests).
    pub fn from_props(props: HashMap<String, String>) -> Self {
        Self {
            path: PathBuf::new(),
            props,
        }
    }

    /// File this config persists to.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Number of stored properties.
    pub fn len(&self) -> usize {
        self.props.len()
    }

    /// True when no properties are stored.
    pub fn is_empty(&self) -> bool {
        self.props.is_empty()
    }

    /// Read-only lookup without default insertion.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.props.get(key).map(String::as_str)
    }

    /// Parse `key=value` text with the exact rules of the C++ `load()`.
    pub fn parse_text(text: &str) -> HashMap<String, String> {
        let mut props = HashMap::new();
        for line in text.lines() {
            if line.is_empty() || line.as_bytes().first() == Some(&b'#') {
                continue;
            }
            let eq = match line.find('=') {
                Some(eq) => eq,
                None => continue,
            };
            let key = trim_trailing_spaces(&line[..eq]);
            let value = trim_leading_spaces(&line[eq + 1..]);
            // `emplace`: first occurrence wins.
            if !props.contains_key(key) {
                props.insert(key.to_string(), value.to_string());
            }
        }
        props
    }

    /// Serialize to the exact `save()` format: header line plus one
    /// `key=value` line per entry.
    pub fn serialize(&self) -> String {
        let mut out = String::from(PROPERTIES_HEADER);
        out.push('\n');
        for (key, value) in &self.props {
            out.push_str(key);
            out.push('=');
            out.push_str(value);
            out.push('\n');
        }
        out
    }

    /// Mirrors `getStringProperty()`: returns the stored value, or inserts
    /// and persists `default` before returning it.
    pub fn get_string(&mut self, key: &str, default: &str) -> String {
        match self.props.get(key) {
            Some(value) => value.clone(),
            None => {
                self.props.insert(key.to_string(), default.to_string());
                self.save();
                default.to_string()
            }
        }
    }

    /// Mirrors `getIntProperty()`: parses via leading-integer rules matching
    /// `std::from_chars` (trailing garbage ignored, overflow is an error).
    /// On parse failure the default is stored in memory (without saving,
    /// exactly like C++) and returned.
    pub fn get_int(&mut self, key: &str, default: i32) -> i32 {
        let fallback = default.to_string();
        let value = self.get_string(key, &fallback);
        match parse_int_prefix(&value) {
            Some(parsed) => parsed,
            None => {
                self.props.insert(key.to_string(), fallback);
                default
            }
        }
    }

    /// Mirrors `getBooleanProperty()`: true only for the exact string
    /// `"true"` (missing keys are inserted with `"true"`/`"false"`).
    pub fn get_bool(&mut self, key: &str, default: bool) -> bool {
        let fallback = if default { "true" } else { "false" };
        self.get_string(key, fallback) == "true"
    }

    fn load(&mut self) {
        let text = match fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(_) => return,
        };
        self.props = Self::parse_text(&text);
    }

    fn save(&self) {
        // Surface save failures: silent loss of server.properties edits
        // (motd, difficulty) is worse than a log line.
        if let Err(e) = fs::write(&self.path, self.serialize()) {
            crate::server_log::warning(&format!("cannot save {}: {e}", self.path.display()));
        }
    }
}

fn trim_trailing_spaces(s: &str) -> &str {
    let bytes = s.as_bytes();
    let mut end = bytes.len();
    while end > 0 && bytes[end - 1] == b' ' {
        end -= 1;
    }
    &s[..end]
}

fn trim_leading_spaces(s: &str) -> &str {
    let bytes = s.as_bytes();
    let mut start = 0;
    while start < bytes.len() && bytes[start] == b' ' {
        start += 1;
    }
    &s[start..]
}

/// Leading-integer parse matching `std::from_chars` for `int`: optional
/// `-`, at least one ASCII digit, trailing characters ignored, overflow or
/// missing digits is an error.
fn parse_int_prefix(s: &str) -> Option<i32> {
    let bytes = s.as_bytes();
    let mut i = 0;
    let mut negative = false;
    if bytes.first() == Some(&b'-') {
        negative = true;
        i = 1;
    }
    if i >= bytes.len() || !bytes[i].is_ascii_digit() {
        return None;
    }
    let mut acc: i32 = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        let digit = (bytes[i] - b'0') as i32;
        if negative {
            acc = acc.checked_mul(10)?.checked_sub(digit)?;
        } else {
            acc = acc.checked_mul(10)?.checked_add(digit)?;
        }
        i += 1;
    }
    Some(acc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn tmp_path(name: &str) -> PathBuf {
        let id = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "server_{}_{}_{}.properties",
            name,
            std::process::id(),
            id
        ))
    }

    fn config_with(props: &[(&str, &str)]) -> ServerConfig {
        let mut map = HashMap::new();
        for (k, v) in props {
            map.insert((*k).to_string(), (*v).to_string());
        }
        ServerConfig {
            path: tmp_path("mem"),
            props: map,
        }
    }

    #[test]
    fn parse_skips_comments_empty_and_bare_lines() {
        let props = ServerConfig::parse_text(
            "#Minecraft server properties\n\nonline-mode=true\nbare line\nport=25565\n",
        );
        assert_eq!(props.get("online-mode").map(String::as_str), Some("true"));
        assert_eq!(props.get("port").map(String::as_str), Some("25565"));
        assert_eq!(props.len(), 2);
    }

    #[test]
    fn parse_trims_key_trailing_and_value_leading_spaces() {
        let props = ServerConfig::parse_text("level-name   =   world\n");
        assert_eq!(props.get("level-name").map(String::as_str), Some("world"));
    }

    #[test]
    fn parse_first_occurrence_wins() {
        let props = ServerConfig::parse_text("a=1\na=2\n");
        assert_eq!(props.get("a").map(String::as_str), Some("1"));
    }

    #[test]
    fn parse_empty_value_and_hash_inside_value() {
        let props = ServerConfig::parse_text("server-ip=\nnote=a#b\n");
        assert_eq!(props.get("server-ip").map(String::as_str), Some(""));
        assert_eq!(props.get("note").map(String::as_str), Some("a#b"));
    }

    #[test]
    fn serialize_parse_roundtrip() {
        let config = config_with(&[
            ("online-mode", "true"),
            ("server-port", "25565"),
            ("level-name", "world"),
            ("motd", "hello world"),
        ]);
        let text = config.serialize();
        assert!(text.starts_with("#Minecraft server properties\n"));
        let reparsed = ServerConfig::parse_text(&text);
        assert_eq!(reparsed, config.props);
    }

    #[test]
    fn get_string_returns_stored_and_inserts_default() {
        let mut config = config_with(&[("a", "1")]);
        assert_eq!(config.get_string("a", "x"), "1");
        assert_eq!(config.get_string("missing", "dflt"), "dflt");
        assert_eq!(config.get("missing"), Some("dflt"));
    }

    #[test]
    fn get_int_valid_invalid_and_overflow() {
        let mut config = config_with(&[
            ("port", "25565"),
            ("bad", "abc"),
            ("big", "9999999999"),
            ("empty", ""),
        ]);
        assert_eq!(config.get_int("port", 0), 25565);
        assert_eq!(config.get_int("bad", 7), 7);
        assert_eq!(config.get("bad"), Some("7"));
        assert_eq!(config.get_int("big", 7), 7);
        assert_eq!(config.get_int("empty", 7), 7);
        // Missing key is inserted with the default.
        assert_eq!(config.get_int("new-key", 42), 42);
        assert_eq!(config.get("new-key"), Some("42"));
    }

    #[test]
    fn get_int_ignores_trailing_garbage_like_from_chars() {
        let mut config = config_with(&[("v", "123abc"), ("neg", "-12x")]);
        assert_eq!(config.get_int("v", 0), 123);
        assert_eq!(config.get_int("neg", 0), -12);
    }

    #[test]
    fn get_bool_matches_exact_true() {
        let mut config = config_with(&[
            ("t", "true"),
            ("f", "false"),
            ("upper", "True"),
            ("one", "1"),
        ]);
        assert!(config.get_bool("t", false));
        assert!(!config.get_bool("f", true));
        assert!(!config.get_bool("upper", true));
        assert!(!config.get_bool("one", true));
        assert!(config.get_bool("absent-true", true));
        assert!(!config.get_bool("absent-false", false));
    }

    #[test]
    fn file_roundtrip_load_save_and_missing_file() {
        // Missing file: generated with header only, then readable.
        let path = tmp_path("gen");
        let _ = fs::remove_file(&path);
        let mut fresh = ServerConfig::open(&path);
        assert!(path.exists());
        assert!(fresh.is_empty());
        assert_eq!(fresh.get_string("server-port", "25565"), "25565");

        // Existing file: loaded, mutated through getters, reloaded intact.
        let mut loaded = ServerConfig::open(&path);
        assert_eq!(loaded.get_string("server-port", "1"), "25565");
        assert_eq!(loaded.get_int("max-players", 20), 20);
        assert!(loaded.get_bool("online-mode", true));

        let reread = ServerConfig::open(&path);
        assert_eq!(reread.get("server-port"), Some("25565"));
        assert_eq!(reread.get("max-players"), Some("20"));
        assert_eq!(reread.get("online-mode"), Some("true"));

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn loads_repo_server_properties_sample() {
        let props = ServerConfig::parse_text(
            "#Minecraft server properties\nserver-port=25565\nonline-mode=true\nlevel-name=world\n",
        );
        assert_eq!(props.get("server-port").map(String::as_str), Some("25565"));
        assert_eq!(props.get("online-mode").map(String::as_str), Some("true"));
        assert_eq!(props.get("level-name").map(String::as_str), Some("world"));
    }
}

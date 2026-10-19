//! Server administration (mirrors Java `ServerConfigurationManager`
//! ban/op/IP lists and vanilla op commands).
//!
//! Two layers:
//! - Name lists: lowercase normalization, `banned-*.txt`/`ops.txt` file
//!   format (trim trailing space/CR/LF, skip empties, sorted+deduplicated
//!   like `std::set`), membership and mutation.
//! - Chat driver: tokenizing, strict argument parsing, permission and range
//!   checks, with effects applied straight to the world and session.
//!
//! Number parsing is deliberately STRICT (trailing garbage rejected, like
//! Java `parseInt`/`parseDouble`): the old C++ `from_chars` accepted
//! prefixes such as `12ab` as `12`. Doubles must additionally be finite
//! (`from_chars` rejects `inf`/`nan`, Rust `parse` does not).

use std::collections::{BTreeSet, HashSet};

use crate::entity::table::Entity;
use crate::session::{PlaySession, SessionBroadcast, pkt_chat};
use crate::world::World;

/// Lowercase ASCII like the C++ `::tolower` loop over latin names.
pub fn admin_normalize(name: &str) -> String {
    name.to_ascii_lowercase()
}

/// Parse a ban/op list file body: trim trailing space/CR/LF per line,
/// skip empties, insert lowercased (sorted+deduplicated via `BTreeSet`,
/// matching `std::set<std::string>` iteration order on save).
pub fn admin_parse_list(body: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for line in body.lines() {
        let trimmed = line.trim_end_matches(['\r', '\n', ' ']);
        if !trimmed.is_empty() {
            out.insert(admin_normalize(trimmed));
        }
    }
    out
}

/// Serialize a list back (one entry per line, trailing newline each).
pub fn admin_render_list(list: &BTreeSet<String>) -> String {
    let mut out = String::new();
    for entry in list {
        out.push_str(entry);
        out.push('\n');
    }
    out
}

/// Strict `from_chars<int>`-shaped parse used by command args: optional
/// sign, ASCII digits, nothing else. Returns the value on full match.
pub fn parse_command_int(s: &str) -> Option<i32> {
    let b = s.as_bytes();
    if b.is_empty() {
        return None;
    }
    let mut i = 0;
    let neg = if b[0] == b'+' || b[0] == b'-' {
        i = 1;
        b[0] == b'-'
    } else {
        false
    };
    if i >= b.len() || !b[i].is_ascii_digit() {
        return None;
    }
    let mut acc: i64 = 0;
    while i < b.len() && b[i].is_ascii_digit() {
        acc = acc * 10 + (b[i] - b'0') as i64;
        if acc > i32::MAX as i64 + 1 {
            return None;
        }
        i += 1;
    }
    if i != b.len() {
        return None;
    }
    let v = if neg { -acc } else { acc };
    if v < i32::MIN as i64 || v > i32::MAX as i64 {
        return None;
    }
    Some(v as i32)
}

/// Strict finite-double parse for command args: `[+-]? (digits [. digits]
/// | . digits) ([eE] [+-]? digits)?`, full match, finite result.
pub fn parse_command_double(s: &str) -> Option<f64> {
    let b = s.as_bytes();
    if b.is_empty() {
        return None;
    }
    let mut i = 0;
    if b[0] == b'+' || b[0] == b'-' {
        i = 1;
    }
    let mut digits = 0;
    while i < b.len() && b[i].is_ascii_digit() {
        digits += 1;
        i += 1;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            digits += 1;
            i += 1;
        }
    }
    if digits == 0 {
        return None;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        i += 1;
        if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
            i += 1;
        }
        let mut exp_digits = 0;
        while i < b.len() && b[i].is_ascii_digit() {
            exp_digits += 1;
            i += 1;
        }
        if exp_digits == 0 {
            return None;
        }
    }
    if i != b.len() {
        return None;
    }
    let v: f64 = s.parse().ok()?;
    if v.is_finite() {
        Some(v)
    } else {
        None
    }
}

/// One chat line back to the issuing player.
fn say(sess: &mut PlaySession, text: &str) {
    sess.outbox.push(pkt_chat(text));
}

/// Parse a named time preset into ticks (0..24000) matching modern Minecraft.
pub fn parse_time_preset(arg: &str) -> Option<i32> {
    match arg.to_ascii_lowercase().as_str() {
        "day" => Some(1000),
        "noon" => Some(6000),
        "sunset" => Some(12000),
        "night" => Some(13000),
        "midnight" => Some(18000),
        "sunrise" => Some(23000),
        _ => None,
    }
}

/// Calculate the target world time for a `/time set <daytime>` command.
/// Matches modern Minecraft: if the target daytime is less than the current
/// daytime, advances to the next day instead of rolling back.
pub fn calculate_set_time(current_time: i64, daytime: i32) -> i64 {
    if daytime >= 24000 {
        daytime as i64
    } else {
        let current_day = current_time.max(0) / 24000;
        let mut target = current_day * 24000 + daytime as i64;
        if target < current_time {
            target += 24000;
        }
        target
    }
}

/// Player `/give` + `/tp` dispatcher (mirrors
/// `NetServerHandler::handleCommand`). `msg` must start with `/`; anything
/// else is ignored. All user-facing strings match the C++ originals.
pub fn chat_command(
    world: &mut World,
    sess: &mut PlaySession,
    ops: &HashSet<String>,
    broadcast: &mut Vec<SessionBroadcast>,
    msg: &str,
) {
    // Ops store lowercased; the query lowercases like C++ isOp.
    let op = match world.entities.get(sess.player) {
        Some(Entity::Player(p)) => ops.contains(&p.username.to_ascii_lowercase()),
        _ => false,
    };
    let body = match msg.strip_prefix('/') {
        Some(b) => b,
        None => return,
    };
    let args: Vec<&str> = body.split_whitespace().collect();
    if args.is_empty() {
        return;
    }
    let cmd = args[0];
    // Vanilla player commands (Java NetServerHandler.func_4010_d): /me
    // emotes to everyone, /kill suicides, /tell whispers. These need no op.
    if cmd.eq_ignore_ascii_case("me") {
        let text = body.split_once(char::is_whitespace).map(|x| x.1).unwrap_or("").trim();
        let username = match world.entities.get(sess.player) {
            Some(Entity::Player(p)) => p.username.clone(),
            _ => return,
        };
        broadcast.push(SessionBroadcast::Chat(format!("* {username} {text}")));
        return;
    }
    if cmd.eq_ignore_ascii_case("kill") {
        world.attack_living(sess.player, 1000, None);
        return;
    }
    if cmd.eq_ignore_ascii_case("tell") {
        if args.len() < 3 {
            return;
        }
        let username = match world.entities.get(sess.player) {
            Some(Entity::Player(p)) => p.username.clone(),
            _ => return,
        };
        // Re-split to keep the raw message (whitespace-preserved).
        let rest = body.split_once(char::is_whitespace).map(|x| x.1).unwrap_or("");
        let message = rest.split_once(char::is_whitespace).map(|x| x.1).unwrap_or("").trim();
        broadcast.push(SessionBroadcast::Tell {
            target: args[1].to_string(),
            text: format!("§7{username} whispers {message}"),
        });
        return;
    }
    if !op && (cmd.eq_ignore_ascii_case("give") || cmd.eq_ignore_ascii_case("tp") || cmd.eq_ignore_ascii_case("seed") || cmd.eq_ignore_ascii_case("boost") || cmd.eq_ignore_ascii_case("time")) {
        say(sess, "You do not have permission to use this command");
        return;
    }
    if cmd == "give" {
        if args.len() < 2 {
            say(sess, "Usage: /give <itemId> [count] [damage]");
            return;
        }
        let (Some(item_id), count, damage) = (
            parse_command_int(args[1]),
            args.get(2).and_then(|s| parse_command_int(s)).unwrap_or(1),
            args.get(3).and_then(|s| parse_command_int(s)).unwrap_or(0),
        )
        else {
            say(sess, "Invalid command arguments");
            return;
        };
        // NOTE: `args.get(2)` on a missing slot yields the default WITHOUT
        // parsing; only present-but-broken tokens error out. Re-check:
        if (args.len() >= 3 && parse_command_int(args[2]).is_none())
            || (args.len() >= 4 && parse_command_int(args[3]).is_none())
        {
            say(sess, "Invalid command arguments");
            return;
        }
        let count = count.clamp(1, 64);
        if item_id <= 0 || item_id >= 32000 || !crate::item_data::item_is_valid(item_id) {
            if (1..256).contains(&item_id) {
                say(sess, &format!("Unknown block id: {item_id}"));
            } else {
                say(sess, "Invalid item id");
            }
            return;
        }
        if let Some(e) = world.entities.get(sess.player) {
            let (px, py, pz) = (e.body().pos[0], e.body().pos[1], e.body().pos[2]);
            world.spawn_item_entity(item_id, count, damage, px, py, pz);
        }
        say(sess, &format!("Gave {count}x {item_id}"));
    } else if cmd == "tp" {
        if args.len() < 4 {
            say(sess, "Usage: /tp <x> <y> <z>");
            return;
        }
        let (Some(tx), Some(ty), Some(tz)) = (
            parse_command_double(args[1]),
            parse_command_double(args[2]),
            parse_command_double(args[3]),
        )
        else {
            say(sess, "Invalid command arguments");
            return;
        };
        let (yaw, pitch) = match world.entities.get(sess.player) {
            Some(e) => (e.body().yaw, e.body().pitch),
            None => (0.0, 0.0),
        };
        sess.teleport_to(world, sess.player, [tx, ty, tz], yaw, pitch);
        // C++ std::to_string(double) prints 6 decimals; match it exactly.
        say(sess, &format!("Teleported to {tx:.6}, {ty:.6}, {tz:.6}"));
    } else if cmd.eq_ignore_ascii_case("boost") {
        let sub = args.get(1).map(|s| s.to_ascii_lowercase()).unwrap_or_default();
        if sub == "on" || sub == "enable" || sub == "1" {
            if !op {
                say(sess, "You do not have permission to toggle GPU boost");
                return;
            }
            if crate::gpu::is_gpu_available() {
                crate::gpu::set_gpu_enabled(true);
                say(sess, &format!("§aGPU boost enabled: {}", crate::gpu::gpu_device_info()));
            } else {
                say(sess, "§cCannot enable GPU boost: no compatible GPU device available");
            }
        } else if sub == "off" || sub == "disable" || sub == "0" {
            if !op {
                say(sess, "You do not have permission to toggle GPU boost");
                return;
            }
            crate::gpu::set_gpu_enabled(false);
            say(sess, "§eGPU boost disabled (CPU default)");
        } else {
            let state = if crate::gpu::is_gpu_enabled() {
                "§aENABLED"
            } else {
                "§7DISABLED (CPU default)"
            };
            let dev = crate::gpu::gpu_device_info();
            say(sess, &format!("GPU boost: {state} §8[{dev}]"));
        }
    } else if cmd.eq_ignore_ascii_case("seed") {
        say(sess, &format!("Seed: {}", world.seed));
    } else if cmd.eq_ignore_ascii_case("time") {
        if args.len() < 2 {
            say(sess, "Usage: /time <set|add|query> <value>");
            return;
        }
        let sub = args[1].to_ascii_lowercase();
        if sub == "set" {
            if args.len() < 3 {
                say(sess, "Usage: /time set <day|noon|sunset|night|midnight|sunrise|value>");
                return;
            }
            let target_daytime = parse_time_preset(args[2])
                .or_else(|| parse_command_int(args[2]).filter(|&v| v >= 0));
            let Some(daytime) = target_daytime else {
                say(sess, "Invalid time value");
                return;
            };
            let new_time = calculate_set_time(world.time, daytime);
            world.time = new_time;
            world.update_skylight_subtracted();
            broadcast.push(SessionBroadcast::TimeChanged(new_time));
            say(sess, &format!("Set the time to {new_time}"));
        } else if sub == "add" {
            if args.len() < 3 {
                say(sess, "Usage: /time add <value>");
                return;
            }
            let Some(to_add) = parse_command_int(args[2]).filter(|&v| v > 0) else {
                say(sess, "Invalid time value");
                return;
            };
            world.time = world.time.saturating_add(to_add as i64);
            world.update_skylight_subtracted();
            broadcast.push(SessionBroadcast::TimeChanged(world.time));
            say(sess, &format!("Added {to_add} to the time"));
        } else if sub == "query" {
            let query_type = args
                .get(2)
                .map(|s| s.to_ascii_lowercase())
                .unwrap_or_else(|| "daytime".to_string());
            let value = match query_type.as_str() {
                "daytime" => world.time.rem_euclid(24000),
                "gametime" => world.time,
                "day" => world.time.max(0) / 24000,
                _ => {
                    say(sess, "Usage: /time query <daytime|gametime|day>");
                    return;
                }
            };
            say(sess, &format!("The time is {value}"));
        } else if let Some(daytime) =
            parse_time_preset(&sub).or_else(|| parse_command_int(&sub).filter(|&v| v >= 0))
        {
            // Shorthand: `/time day`, `/time night`, `/time 1000`
            let new_time = calculate_set_time(world.time, daytime);
            world.time = new_time;
            world.update_skylight_subtracted();
            broadcast.push(SessionBroadcast::TimeChanged(new_time));
            say(sess, &format!("Set the time to {new_time}"));
        } else {
            say(sess, "Usage: /time <set|add|query> <value>");
        }
    } else {
        say(sess, &format!("Unknown command: {cmd}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_roundtrip() {
        assert_eq!(admin_normalize("NotCH"), "notch");
        let parsed = admin_parse_list("Notch\n\n  \nsteve \r\nNOTCH\n");
        let mut expected = BTreeSet::new();
        expected.insert("notch".to_string());
        expected.insert("steve".to_string());
        assert_eq!(parsed, expected);
        assert_eq!(admin_render_list(&parsed), "notch\nsteve\n");
        assert_eq!(admin_render_list(&BTreeSet::new()), "");
    }

    #[test]
    fn test_strict_int() {
        assert_eq!(parse_command_int("5"), Some(5));
        assert_eq!(parse_command_int("-12"), Some(-12));
        assert_eq!(parse_command_int("+7"), Some(7));
        assert_eq!(parse_command_int("12ab"), None);
        assert_eq!(parse_command_int(""), None);
        assert_eq!(parse_command_int("1_0"), None);
        assert_eq!(parse_command_int("9999999999"), None);
    }

    #[test]
    fn test_strict_double() {
        assert_eq!(parse_command_double("1.5"), Some(1.5));
        assert_eq!(parse_command_double("-3"), Some(-3.0));
        assert_eq!(parse_command_double("1e3"), Some(1000.0));
        assert_eq!(parse_command_double("inf"), None);
        assert_eq!(parse_command_double("nan"), None);
        assert_eq!(parse_command_double("1.5x"), None);
        assert_eq!(parse_command_double(""), None);
        assert_eq!(parse_command_double("1_0"), None);
    }

    use crate::entity::table::{Entity, PlayerEnt};
    use crate::session::{PlaySession, SessionBroadcast};
    use crate::world::World;

    fn setup(op: bool) -> (World, PlaySession, HashSet<String>) {
        let mut w = World::new(7);
        let id = w.entities.alloc_id();
        let mut p = PlayerEnt::new(id, "Steve");
        p.living.body.set_position(0.5, 64.0, 0.5);
        p.respawn_ticks = 0;
        w.entities.insert(Entity::Player(p));
        let ops = if op {
            HashSet::from(["steve".to_string()])
        } else {
            HashSet::new()
        };
        (w, PlaySession::new(id), ops)
    }

    /// Chat lines out of a session outbox (id-3 packets, length-prefixed).
    fn chats(sess: &PlaySession) -> Vec<String> {
        let mut out = Vec::new();
        for msg in &sess.outbox {
            if msg.first() == Some(&3) && msg.len() >= 3 {
                let n = u16::from_be_bytes([msg[1], msg[2]]) as usize;
                if msg.len() >= 3 + n {
                    out.push(String::from_utf8_lossy(&msg[3..3 + n]).into_owned());
                }
            }
        }
        out
    }

    fn run(w: &mut World, sess: &mut PlaySession, ops: &HashSet<String>, cmd: &str) -> Vec<SessionBroadcast> {
        sess.outbox.clear();
        let mut bc = Vec::new();
        chat_command(w, sess, ops, &mut bc, cmd);
        bc
    }

    fn gave_item(w: &World, item_id: i32, count: i32, damage: i32) -> bool {
        w.entities.alive_ids().iter().any(|oid| {
            matches!(w.entities.get(*oid),
                Some(Entity::Item(e)) if e.item_id == item_id && e.count == count && e.damage == damage)
        })
    }

    #[test]
    fn test_chat_driver() {
        // Non-command input ignored.
        let (mut w, mut sess, ops) = setup(true);
        run(&mut w, &mut sess, &ops, "hello");
        assert!(chats(&sess).is_empty());

        // Permission gate.
        let (mut w, mut sess, ops) = setup(false);
        run(&mut w, &mut sess, &ops, "/give 5");
        assert_eq!(chats(&sess), vec!["You do not have permission to use this command"]);
        run(&mut w, &mut sess, &ops, "/seed");
        assert_eq!(chats(&sess), vec!["You do not have permission to use this command"]);
        run(&mut w, &mut sess, &ops, "/boost");
        assert_eq!(chats(&sess), vec!["You do not have permission to use this command"]);
        run(&mut w, &mut sess, &ops, "/boost on");
        assert_eq!(chats(&sess), vec!["You do not have permission to use this command"]);

        // Boost command for ops.
        let (mut w, mut sess, ops) = setup(true);
        run(&mut w, &mut sess, &ops, "/boost");
        assert!(chats(&sess)[0].starts_with("GPU boost:"));

        // Seed command for ops.
        let (mut w, mut sess, ops) = setup(true);
        w.seed = 9876543210;
        run(&mut w, &mut sess, &ops, "/seed");
        assert_eq!(chats(&sess), vec!["Seed: 9876543210"]);

        // Give happy path with clamping.
        let (mut w, mut sess, ops) = setup(true);
        run(&mut w, &mut sess, &ops, "/give 5 99 2");
        assert!(gave_item(&w, 5, 64, 2));
        assert_eq!(chats(&sess), vec!["Gave 64x 5"]);

        // Give validation errors.
        let (mut w, mut sess, ops) = setup(true);
        run(&mut w, &mut sess, &ops, "/give");
        assert_eq!(chats(&sess), vec!["Usage: /give <itemId> [count] [damage]"]);
        run(&mut w, &mut sess, &ops, "/give 0");
        assert_eq!(chats(&sess), vec!["Invalid item id"]);
        run(&mut w, &mut sess, &ops, "/give 200");
        assert_eq!(chats(&sess), vec!["Unknown block id: 200"]);
        run(&mut w, &mut sess, &ops, "/give 5x");
        assert_eq!(chats(&sess), vec!["Invalid command arguments"]);

        // Tp happy path (6-decimal message like to_string).
        run(&mut w, &mut sess, &ops, "/tp 10 64 -3");
        match w.entities.get(sess.player).unwrap() {
            Entity::Player(p) => assert_eq!(p.living.body.pos, [10.0, 64.0, -3.0]),
            _ => unreachable!(),
        }
        assert_eq!(chats(&sess), vec!["Teleported to 10.000000, 64.000000, -3.000000"]);
        run(&mut w, &mut sess, &ops, "/tp 10 64");
        assert_eq!(chats(&sess), vec!["Usage: /tp <x> <y> <z>"]);
        run(&mut w, &mut sess, &ops, "/tp 10 oo 3");
        assert_eq!(chats(&sess), vec!["Invalid command arguments"]);

        // Unknown command.
        run(&mut w, &mut sess, &ops, "/dance");
        assert_eq!(chats(&sess), vec!["Unknown command: dance"]);

        // /me emotes to broadcast (no op needed).
        let (mut w, mut sess, ops) = setup(false);
        let bc = run(&mut w, &mut sess, &ops, "/me does a flip");
        assert!(bc.iter().any(|b| matches!(b, SessionBroadcast::Chat(t) if t == "* Steve does a flip")));

        // /kill damages self to death.
        let (mut w, mut sess, ops) = setup(false);
        let _ = run(&mut w, &mut sess, &ops, "/kill");
        match w.entities.get(sess.player).unwrap() {
            Entity::Player(p) => assert!(p.living.health <= 0),
            _ => unreachable!(),
        }

        // /tell queues a targeted whisper.
        let (mut w, mut sess, ops) = setup(false);
        let bc = run(&mut w, &mut sess, &ops, "/tell Notch hello there");
        assert!(bc.iter().any(|b| matches!(b, SessionBroadcast::Tell { target, text }
            if target == "Notch" && text.contains("whispers"))));
    }

    #[test]
    fn test_time_command() {
        // Permission check
        let (mut w, mut sess, ops) = setup(false);
        run(&mut w, &mut sess, &ops, "/time set day");
        assert_eq!(chats(&sess), vec!["You do not have permission to use this command"]);

        // Op can run /time set day
        let (mut w, mut sess, ops) = setup(true);
        let bc = run(&mut w, &mut sess, &ops, "/time set day");
        assert_eq!(w.time, 1000);
        assert_eq!(chats(&sess), vec!["Set the time to 1000"]);
        assert!(bc.iter().any(|b| matches!(b, SessionBroadcast::TimeChanged(1000))));

        // Day presets
        run(&mut w, &mut sess, &ops, "/time set noon");
        assert_eq!(w.time, 6000);
        run(&mut w, &mut sess, &ops, "/time set sunset");
        assert_eq!(w.time, 12000);
        run(&mut w, &mut sess, &ops, "/time set night");
        assert_eq!(w.time, 13000);
        run(&mut w, &mut sess, &ops, "/time set midnight");
        assert_eq!(w.time, 18000);
        run(&mut w, &mut sess, &ops, "/time set sunrise");
        assert_eq!(w.time, 23000);

        // Next day advance: current is 23000, setting day (1000) advances to 25000
        run(&mut w, &mut sess, &ops, "/time set day");
        assert_eq!(w.time, 25000);
        assert_eq!(chats(&sess), vec!["Set the time to 25000"]);

        // Explicit ticks
        run(&mut w, &mut sess, &ops, "/time set 50000");
        assert_eq!(w.time, 50000);

        // /time add
        let bc = run(&mut w, &mut sess, &ops, "/time add 500");
        assert_eq!(w.time, 50500);
        assert_eq!(chats(&sess), vec!["Added 500 to the time"]);
        assert!(bc.iter().any(|b| matches!(b, SessionBroadcast::TimeChanged(50500))));

        // /time query
        run(&mut w, &mut sess, &ops, "/time query daytime");
        assert_eq!(chats(&sess), vec![format!("The time is {}", 50500 % 24000)]);
        run(&mut w, &mut sess, &ops, "/time query gametime");
        assert_eq!(chats(&sess), vec!["The time is 50500"]);
        run(&mut w, &mut sess, &ops, "/time query day");
        assert_eq!(chats(&sess), vec![format!("The time is {}", 50500 / 24000)]);

        // Shorthand syntax
        run(&mut w, &mut sess, &ops, "/time night");
        assert_eq!(w.time.rem_euclid(24000), 13000);

        // Error cases
        run(&mut w, &mut sess, &ops, "/time");
        assert_eq!(chats(&sess), vec!["Usage: /time <set|add|query> <value>"]);
        run(&mut w, &mut sess, &ops, "/time set");
        assert_eq!(chats(&sess), vec!["Usage: /time set <day|noon|sunset|night|midnight|sunrise|value>"]);
        run(&mut w, &mut sess, &ops, "/time set invalid");
        assert_eq!(chats(&sess), vec!["Invalid time value"]);
        run(&mut w, &mut sess, &ops, "/time add -10");
        assert_eq!(chats(&sess), vec!["Invalid time value"]);
        run(&mut w, &mut sess, &ops, "/time query invalid");
        assert_eq!(chats(&sess), vec!["Usage: /time query <daytime|gametime|day>"]);
    }
}

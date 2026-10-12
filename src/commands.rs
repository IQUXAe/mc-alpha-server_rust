#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsoleCommandTag {
    Help,
    List,
    Stop,
    SaveAll,
    SaveOff,
    SaveOn,
    Op,
    Deop,
    BanIp,
    PardonIp,
    Ban,
    Pardon,
    Kick,
    Tp,
    Give,
    Summon,
    Say,
    Tell,
    Boost,
    Seed,
    Unknown,
}

#[derive(Clone, Debug)]
pub struct ParsedCommand {
    pub tag: ConsoleCommandTag,
    pub arg1: String,
    pub arg2: String,
    pub count: i32,
}

fn trim_left(s: &str) -> &str {
    s.trim_start_matches(' ')
}

fn split_two(s: &str) -> (&str, &str) {
    match s.find(' ') {
        Some(idx) => {
            let first = &s[..idx];
            let second = trim_left(&s[idx + 1..]);
            (first, second)
        }
        None => (s, ""),
    }
}

fn arg_of(sv: &str, prefix_len: usize) -> &str {
    if prefix_len >= sv.len() {
        ""
    } else {
        trim_left(&sv[prefix_len..])
    }
}

pub fn parse_console_command(cmd: &str) -> ParsedCommand {
    if cmd.is_empty() {
        return ParsedCommand {
            tag: ConsoleCommandTag::Unknown,
            arg1: String::new(),
            arg2: String::new(),
            count: 1,
        };
    }

    let lower = cmd.to_lowercase();

    let mut tag = ConsoleCommandTag::Unknown;
    let mut arg1 = "";
    let mut arg2 = "";
    let mut count = 1;

    let verb = lower.split_whitespace().next().unwrap_or("");

    if verb == "help" || verb == "?" {
        tag = ConsoleCommandTag::Help;
    } else if verb == "list" {
        tag = ConsoleCommandTag::List;
    } else if verb == "stop" {
        tag = ConsoleCommandTag::Stop;
    } else if verb == "save-all" {
        tag = ConsoleCommandTag::SaveAll;
    } else if verb == "save-off" {
        tag = ConsoleCommandTag::SaveOff;
    } else if verb == "save-on" {
        tag = ConsoleCommandTag::SaveOn;
    } else if verb == "op" {
        tag = ConsoleCommandTag::Op;
        arg1 = arg_of(cmd, 2);
    } else if verb == "deop" {
        tag = ConsoleCommandTag::Deop;
        arg1 = arg_of(cmd, 4);
    } else if verb == "ban-ip" {
        tag = ConsoleCommandTag::BanIp;
        arg1 = arg_of(cmd, 6);
    } else if verb == "pardon-ip" {
        tag = ConsoleCommandTag::PardonIp;
        arg1 = arg_of(cmd, 9);
    } else if verb == "ban" {
        tag = ConsoleCommandTag::Ban;
        arg1 = arg_of(cmd, 3);
    } else if verb == "pardon" {
        tag = ConsoleCommandTag::Pardon;
        arg1 = arg_of(cmd, 6);
    } else if verb == "kick" {
        tag = ConsoleCommandTag::Kick;
        arg1 = arg_of(cmd, 4);
    } else if verb == "tp" {
        tag = ConsoleCommandTag::Tp;
        let rest = arg_of(cmd, 2);
        let (p1, p2) = split_two(rest);
        arg1 = p1;
        arg2 = p2;
    } else if verb == "say" {
        tag = ConsoleCommandTag::Say;
        arg1 = arg_of(cmd, 3);
    } else if verb == "tell" {
        tag = ConsoleCommandTag::Tell;
        let rest = arg_of(cmd, 4);
        let (target, msg) = split_two(rest);
        arg1 = target;
        arg2 = msg;
    } else if verb == "give" {
        // Java: give <player> <id> [count]. count clamps 1..64.
        tag = ConsoleCommandTag::Give;
        let rest = arg_of(cmd, 4);
        let (target, tail) = split_two(rest);
        arg1 = target;
        let (id_str, count_str) = split_two(tail);
        arg2 = id_str;
        if !count_str.is_empty() {
            if let Ok(c) = count_str.parse::<i32>() {
                count = c.clamp(1, 64);
            }
        }
    } else if verb == "summon" {
        tag = ConsoleCommandTag::Summon;
        let rest = arg_of(cmd, 6);
        let (entity_name, tail) = split_two(rest);
        arg1 = entity_name;

        if !tail.is_empty() {
            let (arg1_str, arg2_str) = split_two(tail);
            if !arg1_str.is_empty() {
                let is_number = arg1_str.chars().all(|c| c.is_ascii_digit());
                if is_number {
                    if let Ok(c) = arg1_str.parse::<i32>() {
                        count = c.clamp(1, 64);
                    }
                    arg2 = arg2_str;
                } else {
                    arg2 = arg1_str;
                }
            }
        }
    } else if verb == "boost" {
        tag = ConsoleCommandTag::Boost;
        arg1 = arg_of(cmd, 5);
    } else if verb == "seed" {
        tag = ConsoleCommandTag::Seed;
    }

    ParsedCommand {
        tag,
        arg1: arg1.to_owned(),
        arg2: arg2.to_owned(),
        count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_seed() {
        let cmd = parse_console_command("seed");
        assert_eq!(cmd.tag, ConsoleCommandTag::Seed);
        let cmd_upper = parse_console_command("SEED");
        assert_eq!(cmd_upper.tag, ConsoleCommandTag::Seed);
    }

    #[test]
    fn test_strict_prefix() {
        let cmd = parse_console_command("stopEverything");
        assert_eq!(cmd.tag, ConsoleCommandTag::Unknown);
        let cmd = parse_console_command("stop");
        assert_eq!(cmd.tag, ConsoleCommandTag::Stop);
        let cmd = parse_console_command("listUsers");
        assert_eq!(cmd.tag, ConsoleCommandTag::Unknown);
        let cmd = parse_console_command("list");
        assert_eq!(cmd.tag, ConsoleCommandTag::List);
        let cmd = parse_console_command("op notch");
        assert_eq!(cmd.tag, ConsoleCommandTag::Op);
        assert_eq!(cmd.arg1, "notch");
    }
}

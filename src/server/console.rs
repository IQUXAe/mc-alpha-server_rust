//! Console commands: `handle_console` plus its helpers and `parse_console`.
//! Split out of `server.rs`; behavior unchanged.

use std::collections::BTreeSet;
use crate::commands::{ConsoleCommandTag, parse_console_command};
use crate::entity::table::{AnimalEnt, AnimalKind, Entity, EntityId, MobEnt, MobKind};
use crate::server::sessions::SessionState;
use crate::server::settings::write_list;
use crate::server::Server;
use crate::server_admin::admin_normalize;
use crate::server_constants::AUTO_SAVE_INTERVAL_TICKS_DEFAULT;
use crate::server_log as log;
use crate::session::{pkt_chat, pkt_kick};

impl Server {
    /// Kick a player by name (mirrors the console `kick`).
    fn kick_player(&mut self, name: &str, reason: &str) -> bool {
        let lower = admin_normalize(name);
        let found =
            self.players
                .iter()
                .find_map(|(eid, cid)| match self.world.entities.get(*eid) {
                    Some(Entity::Player(p)) if admin_normalize(&p.username) == lower => {
                        Some((*eid, *cid))
                    }
                    _ => None,
                });
        match found {
            Some((eid, cid)) => {
                if let Some(mut sess) = self.sessions.remove(&cid) {
                    let held = match &mut sess.state {
                        SessionState::Play(play, _) => {
                            play.outbox.push(pkt_kick(reason));
                            play.held_id
                        }
                        SessionState::Login(_) => 0,
                    };
                    sess.flush();
                    log::info(&format!("Disconnecting {name}: {reason}"));
                    // A pre-join kick has no player row to log out.
                    if matches!(sess.state, SessionState::Play(_, _)) {
                        self.logout(eid, held, false);
                    }
                    self.remove_session(cid, sess);
                }
                true
            }
            None => false,
        }
    }
}

impl Server {
    /// Console line entry point (mirrors `handleCommand`).
    pub(crate) fn handle_console(&mut self, line: &str) {
        let (tag, arg1, arg2, count) = parse_console(line);
        match tag {
            ConsoleCommandTag::Help => {
                log::info(
                    "Console commands:\n   help  or  ?               shows this message\n   kick <player>             removes a player from the server\n   ban <player>              bans a player from the server\n   pardon <player>           pardons a banned player\n   ban-ip <ip>               bans an IP address\n   pardon-ip <ip>            pardons a banned IP address\n   op <player>               turns a player into an op\n   deop <player>             removes op status\n   tp <player1> <player2>    teleports player1 to player2\n   give <player> <id> [num]  gives a player a resource\n   summon <mob> [count] [player] spawns debug mobs near player\n   tell <player> <message>   sends a private message\n   stop                      gracefully stops the server\n   save-all                  forces a server-wide level save\n   list                      lists all connected players\n   say <message>             broadcasts a message",
                );
            }
            ConsoleCommandTag::List => {
                log::info(&format!("Connected players: {}", self.player_list()));
            }
            ConsoleCommandTag::Stop => {
                log::info("Stopping the server..");
                self.running = false;
            }
            ConsoleCommandTag::SaveAll => {
                log::info("Forcing save..");
                self.save_world();
                self.save_players();
                log::info("Save complete.");
            }
            ConsoleCommandTag::SaveOff => {
                self.settings.auto_save_interval = 0;
                log::info("Automatic saving is now disabled.");
            }
            ConsoleCommandTag::SaveOn => {
                self.settings.auto_save_interval = AUTO_SAVE_INTERVAL_TICKS_DEFAULT;
                log::info("Automatic saving is now enabled.");
            }
            ConsoleCommandTag::Give => {
                self.console_give(&arg1, &arg2, count);
            }
            ConsoleCommandTag::Op => {
                self.ops.insert(admin_normalize(&arg1));
                self.save_ops();
                log::info(&format!("Opping {arg1}"));
                self.send_chat_to_player(&arg1, "§eYou are now op!");
            }
            ConsoleCommandTag::Deop => {
                self.ops.remove(&admin_normalize(&arg1));
                self.save_ops();
                log::info(&format!("De-opping {arg1}"));
                self.send_chat_to_player(&arg1, "§eYou are no longer op!");
            }
            ConsoleCommandTag::BanIp => {
                self.banned_ips.insert(admin_normalize(&arg1));
                write_list(&self.banned_ips_path, &self.banned_ips);
                log::info(&format!("Banning ip {arg1}"));
            }
            ConsoleCommandTag::PardonIp => {
                self.banned_ips.remove(&admin_normalize(&arg1));
                write_list(&self.banned_ips_path, &self.banned_ips);
                log::info(&format!("Pardoning ip {arg1}"));
            }
            ConsoleCommandTag::Ban => {
                self.banned_players.insert(admin_normalize(&arg1));
                write_list(&self.banned_players_path, &self.banned_players);
                log::info(&format!("Banning {arg1}"));
                self.kick_player(&arg1, "Banned by admin");
            }
            ConsoleCommandTag::Pardon => {
                self.banned_players.remove(&admin_normalize(&arg1));
                write_list(&self.banned_players_path, &self.banned_players);
                log::info(&format!("Pardoning {arg1}"));
            }
            ConsoleCommandTag::Kick => {
                if self.kick_player(&arg1, "Kicked by admin") {
                    log::info(&format!("Kicking {arg1}"));
                } else {
                    log::info(&format!("Can't find user {arg1}. No kick."));
                }
            }
            ConsoleCommandTag::Tp => {
                let (a, b) = (self.entity_named(&arg1), self.entity_named(&arg2));
                match (a, b) {
                    (None, _) => log::info(&format!("Can't find user {arg1}. No tp.")),
                    (_, None) => log::info(&format!("Can't find user {arg2}. No tp.")),
                    (Some(e1), Some(e2)) => {
                        let (pos, yaw, pitch) = match self.world.entities.get(e2) {
                            Some(e) => (e.body().pos, e.body().yaw, e.body().pitch),
                            None => return,
                        };
                        if let Some(cid) = self.players.get(&e1).copied() {
                            if let Some(sess) = self.sessions.get_mut(&cid) {
                                if let SessionState::Play(play, _) = &mut sess.state {
                                    play.teleport_to(&mut self.world, e1, pos, yaw, pitch);
                                }
                            }
                        }
                        log::info(&format!("Teleporting {arg1} to {arg2}."));
                    }
                }
            }
            ConsoleCommandTag::Summon => {
                self.console_summon(&arg1, count, &arg2);
            }
            ConsoleCommandTag::Say => {
                log::info(&format!("[Server] {arg1}"));
                self.broadcast_chat(format!("§d[Server] {arg1}"));
            }
            ConsoleCommandTag::Tell => {
                log::info(&format!("[CONSOLE->{arg1}] {arg2}"));
                if !self.send_chat_to_player(&arg1, &format!("§7CONSOLE whispers {arg2}")) {
                    log::info("There's no player by that name online.");
                }
            }
            ConsoleCommandTag::Unknown => {
                log::info("Unknown console command. Type \"help\" for help.");
            }
        }
    }
}

impl Server {
    /// Comma-joined online names, sorted for determinism (C++ walks its
    /// join-ordered vector; order here is cosmetic admin output).
    fn player_list(&self) -> String {
        let mut names: Vec<String> = self
            .players
            .keys()
            .filter_map(|eid| match self.world.entities.get(*eid) {
                Some(Entity::Player(p)) => Some(p.username.clone()),
                _ => None,
            })
            .collect();
        names.sort();
        names.join(", ")
    }

    /// Chat to one named player (mirrors `sendChatToPlayer`).
    fn send_chat_to_player(&mut self, name: &str, msg: &str) -> bool {
        let bytes = pkt_chat(msg);
        match self.entity_named(name).and_then(|eid| self.players.get(&eid).copied()) {
            Some(cid) => match self.sessions.get(&cid) {
                Some(sess) => {
                    sess.conn.send(bytes);
                    true
                }
                None => false,
            },
            None => false,
        }
    }

    /// Player entity by case-insensitive name (mirrors `getPlayerEntity`).
    pub(crate) fn entity_named(&self, name: &str) -> Option<EntityId> {
        let lower = admin_normalize(name);
        self.players.keys().find_map(|eid| match self.world.entities.get(*eid) {
            Some(Entity::Player(p)) if admin_normalize(&p.username) == lower => Some(*eid),
            _ => None,
        })
    }

    fn save_ops(&self) {
        let ordered: BTreeSet<String> = self.ops.iter().cloned().collect();
        write_list(&self.ops_path, &ordered);
    }

    /// Console give (mirrors `give <player> <id> [count]`): validates the
    /// id, merges through the shared inventory path, syncs the session.
    fn console_give(&mut self, target: &str, id_raw: &str, count: i32) {
        let eid = match self.entity_named(target) {
            Some(e) => e,
            None => {
                log::info(&format!("Can't find user {target}. No give."));
                return;
            }
        };
        let id: i32 = match id_raw.parse() {
            Ok(v) => v,
            Err(_) => {
                log::info(&format!("Invalid item id {id_raw}."));
                return;
            }
        };
        if !crate::item_data::item_is_valid(id) {
            log::info(&format!("Invalid item id {id}."));
            return;
        }
        let count = count.clamp(1, 64);
        let rem = self.world.player_add_item(
            eid,
            crate::inventory::ItemStack::new(id, count, 0),
        );
        let given = count - rem;
        if let Some(cid) = self.players.get(&eid).copied() {
            if let Some(sess) = self.sessions.get_mut(&cid) {
                if let SessionState::Play(play, _) = &mut sess.state {
                    play.send_inventory(&self.world);
                }
            }
        }
        if rem > 0 {
            log::info(&format!("Gave {given} of {id} to {target} ({rem} did not fit)."));
        } else {
            log::info(&format!("Gave {given} of {id} to {target}."));
        }
    }

    /// Debug spawner (mirrors the console `summon`): ring placement
    /// around the anchor with ground clamp and random yaw.
    fn console_summon(&mut self, raw_name: &str, count: i32, target: &str) {
        if raw_name.is_empty() {
            log::info("Usage: summon <mob> [count] [player]");
            return;
        }
        let anchor = if !target.is_empty() {
            match self.entity_named(target) {
                Some(e) => e,
                None => {
                    log::info(&format!("Can't find user {target}. No summon."));
                    return;
                }
            }
        } else if let Some(oldest) = self.players.keys().copied().min() {
            oldest
        } else {
            log::info("No online players to anchor summon.");
            return;
        };
        let name = raw_name.to_ascii_lowercase();
        enum Kind {
            Mob(MobKind),
            Animal(AnimalKind),
        }
        let kind = match name.as_str() {
            "pig" => Kind::Animal(AnimalKind::Pig),
            "sheep" => Kind::Animal(AnimalKind::Sheep),
            "cow" => Kind::Animal(AnimalKind::Cow),
            "chicken" => Kind::Animal(AnimalKind::Chicken),
            "zombie" => Kind::Mob(MobKind::Zombie),
            "skeleton" => Kind::Mob(MobKind::Skeleton),
            "spider" => Kind::Mob(MobKind::Spider),
            "creeper" => Kind::Mob(MobKind::Creeper),
            _ => {
                log::info(&format!(
                    "Unknown mob {raw_name}. Try pig/sheep/cow/chicken/zombie/skeleton/spider/creeper."
                ));
                return;
            }
        };
        let apos = match self.world.entities.get(anchor) {
            Some(e) => e.body().pos,
            None => return,
        };
        let anchor_name = match self.world.entities.get(anchor) {
            Some(Entity::Player(p)) => p.username.clone(),
            _ => String::new(),
        };
        let count = count.clamp(1, 64);
        for i in 0..count {
            let angle = (i as f64 / count.max(1) as f64) * std::f64::consts::TAU;
            let radius = 2.0 + (i % 3) as f64;
            let sx = apos[0] + angle.cos() * radius;
            let sz = apos[2] + angle.sin() * radius;
            let ground = self.world.get_height_value(sx.floor() as i32, sz.floor() as i32) as f64;
            let sy = apos[1].max(ground);
            let id = self.world.entities.alloc_id();
            let yaw = self.world.rng_next_f32() * 360.0;
            match kind {
                Kind::Mob(k) => {
                    let mut m = MobEnt::new(id, k);
                    m.living.body.set_position(sx, sy, sz);
                    m.living.body.yaw = yaw;
                    self.world.entities.insert(Entity::Mob(m));
                }
                Kind::Animal(k) => {
                    let mut a = AnimalEnt::new(id, k);
                    a.living.body.set_position(sx, sy, sz);
                    a.living.body.yaw = yaw;
                    self.world.entities.insert(Entity::Animal(a));
                }
            }
        }
        log::info(&format!("Spawned {count} {raw_name} near {anchor_name}."));
    }
}

fn parse_console(line: &str) -> (ConsoleCommandTag, String, String, i32) {
    let parsed = parse_console_command(line);
    (parsed.tag, parsed.arg1, parsed.arg2, parsed.count)
}

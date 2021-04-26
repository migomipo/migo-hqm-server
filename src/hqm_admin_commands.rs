use crate::hqm_server::{HQMServer, HQMMuteStatus};

use tracing::info;
use std::net::SocketAddr;

impl HQMServer {
    pub(crate) fn admin_deny_message (& mut self, player_index: usize) {
        let msg = format!("Please log in before using that command");
        self.add_directed_server_chat_message(msg,player_index);
    }

    pub(crate) fn set_allow_join (& mut self, player_index: usize, allowed: bool) {
        if let Some(player) = & self.players[player_index] {
            if player.is_admin{
                self.allow_join=allowed;

                if allowed {
                    info!("{} ({}) enabled joins", player.player_name, player_index);
                    let msg = format!("Joins enabled by {}",player.player_name);
                    self.add_server_chat_message(msg);
                } else {
                    info!("{} ({}) disabled joins", player.player_name, player_index);
                    let msg = format!("Joins disabled by {}",player.player_name);
                    self.add_server_chat_message(msg);
                }

            } else {
                self.admin_deny_message(player_index);
            }
        }
    }

    pub(crate) fn mute_player (& mut self, admin_player_index: usize, mute_player_index: usize) {
        if let Some(admin_player) = & self.players[admin_player_index] {
            if admin_player.is_admin {
                let admin_player_name = admin_player.player_name.clone();

                if mute_player_index < self.players.len() {
                    if let Some(mute_player) = & mut self.players[mute_player_index] {
                        mute_player.is_muted = HQMMuteStatus::Muted;
                        info!("{} ({}) muted {} ({})", admin_player_name, admin_player_index, mute_player.player_name, mute_player_index);
                        let msg = format!("{} muted by {}", mute_player.player_name ,admin_player_name);
                        self.add_server_chat_message(msg);
                    }
                }

            } else {
                self.admin_deny_message(admin_player_index);
            }
        }

    }

    pub(crate) fn unmute_player (& mut self, admin_player_index: usize, mute_player_index: usize) {
        if let Some(admin_player) = & self.players[admin_player_index] {
            if admin_player.is_admin {
                let admin_player_name = admin_player.player_name.clone();

                if mute_player_index < self.players.len() {
                    if let Some(mute_player) = & mut self.players[mute_player_index] {
                        let old_status = mute_player.is_muted;
                        mute_player.is_muted = HQMMuteStatus::NotMuted;
                        info!("{} ({}) unmuted {} ({})", admin_player_name, admin_player_index, mute_player.player_name, mute_player_index);
                        let msg = format!("{} unmuted by {}", mute_player.player_name ,admin_player_name);
                        if old_status == HQMMuteStatus::Muted {
                            self.add_server_chat_message(msg);
                        } else {
                            self.add_directed_server_chat_message(msg, admin_player_index);
                        }

                    }
                }

            } else {
                self.admin_deny_message(admin_player_index);
            }
        }
    }

    #[allow(dead_code)]
    pub(crate) fn shadowmute_player (& mut self, admin_player_index: usize, mute_player_index: usize) {
        if let Some(admin_player) = & self.players[admin_player_index] {
            if admin_player.is_admin {
                let admin_player_name = admin_player.player_name.clone();

                if mute_player_index < self.players.len() {
                    if let Some(mute_player) = & mut self.players[mute_player_index] {
                        let old_status = mute_player.is_muted;
                        mute_player.is_muted = HQMMuteStatus::ShadowMuted;
                        info!("{} ({}) shadowmuted {} ({})", admin_player_name, admin_player_index, mute_player.player_name, mute_player_index);
                        let msg = format!("{} shadowmuted by {}", mute_player.player_name ,admin_player_name);
                        if old_status == HQMMuteStatus::Muted {
                            // Fake "unmuting" message
                            let msg = format!("{} unmuted by {}", mute_player.player_name ,admin_player_name);
                            self.add_directed_server_chat_message(msg, mute_player_index);
                        }
                        self.add_directed_server_chat_message(msg, admin_player_index);
                    }
                }
            } else {
                self.admin_deny_message(admin_player_index);
            }
        }
    }

    pub(crate) fn mute_chat (& mut self, player_index: usize) {
        if let Some(player) = & self.players[player_index] {
            if player.is_admin{
                self.is_muted=true;

                let msg = format!("Chat muted by {}",player.player_name);
                info!("{} ({}) muted chat", player.player_name, player_index);
                self.add_server_chat_message(msg);
            } else {
                self.admin_deny_message(player_index);
            }
        }
    }

    pub(crate) fn unmute_chat (& mut self, player_index: usize) {
        if let Some(player) = & self.players[player_index] {
            if player.is_admin{
                self.is_muted=false;

                let msg = format!("Chat unmuted by {}",player.player_name);
                info!("{} ({}) unmuted chat", player.player_name, player_index);

                self.add_server_chat_message(msg);
            } else {
                self.admin_deny_message(player_index);
            }
        }
    }

    pub(crate) fn force_player_off_ice (& mut self, admin_player_index: usize, force_player_index: usize) {

        if let Some(player) = & self.players[admin_player_index] {
            if player.is_admin {
                let admin_player_name = player.player_name.clone();

                if force_player_index < self.players.len() {
                    if self.move_to_spectator(force_player_index) {
                        if let Some(force_player) = & mut self.players[force_player_index] {
                            force_player.team_switch_timer = 500; // 500 ticks, 5 seconds
                            let force_player_name = force_player.player_name.clone();
                            let msg = format!("{} forced off ice by {}",force_player_name,admin_player_name);
                            info!("{} ({}) forced {} ({}) off ice", admin_player_name, admin_player_index, force_player.player_name, force_player_index);
                            self.add_server_chat_message(msg);
                        }
                    }
                }
            } else {
                self.admin_deny_message(admin_player_index);
                return;
            }
        }

    }

    pub(crate) fn set_preferred_faceoff_position(& mut self, player_index: usize, input_position:&str) {
        let input_position = input_position.to_uppercase();
        if self.game.world.rink.allowed_positions.contains(& input_position) {
            if let Some(player) = & mut self.players[player_index] {
                info!("{} ({}) set position {}", player.player_name, player_index, input_position);
                let msg = format!("{} position {}", player.player_name, input_position);

                player.preferred_faceoff_position = Some(input_position);
                self.add_server_chat_message(msg);

            }
        }
    }

    pub(crate) fn admin_login (& mut self, player_index: usize, password:&str) {
        if let Some(player) = & mut self.players[player_index] {

            if self.config.password == password{
                player.is_admin = true;
                info!("{} ({}) is now admin", player.player_name, player_index);
                let msg = format!("{} admin", player.player_name);
                self.add_server_chat_message(msg);
            } else {
                info!("{} ({}) tried to become admin, entered wrong password", player.player_name, player_index);
                let msg = format!("Incorrect password");
                self.add_directed_server_chat_message(msg,player_index);

            }
        }
    }

    pub(crate) fn kick_all_matching (& mut self, admin_player_index: usize, kick_player_name: &str, ban_player: bool) {

        if let Some(player) = & self.players[admin_player_index]{
            if player.is_admin {
                let admin_player_name = player.player_name.clone();

                // 0 full string | 1 begins with | 2 ends with | 3 contains
                let match_mode = if kick_player_name.starts_with("%"){
                    if kick_player_name.ends_with("%"){
                        3// %contains%
                    }else{
                        2// %ends with
                    }
                }else if kick_player_name.ends_with("%"){
                    1// begins with%
                } else {
                    0
                };

                // Because we allow matching using wildcards, we use vectors for multiple instances found
                let mut kick_player_list: Vec<(usize, String, SocketAddr)> = Vec::new();

                for (player_index, p) in self.players.iter_mut().enumerate() {
                    if let Some(player) = p {

                        match match_mode {
                            0 => { // full string
                                if player.player_name == kick_player_name{
                                    kick_player_list.push((player_index, player.player_name.clone(), player.addr));
                                }
                            },
                            1 => { // begins with%
                                let match_string: String = kick_player_name.chars().take(kick_player_name.len()-1).collect();

                                if player.player_name.starts_with(&match_string) || player.player_name == kick_player_name{
                                    kick_player_list.push((player_index, player.player_name.clone(), player.addr));
                                }
                            },
                            2 => { // %ends with
                                let match_string: String = kick_player_name.chars().skip(1).take(kick_player_name.len()-1).collect();

                                if player.player_name.ends_with(&match_string) || player.player_name == kick_player_name{
                                    kick_player_list.push((player_index, player.player_name.clone(), player.addr));
                                }
                            },
                            3 => { // %contains%
                                let match_string: String = kick_player_name.chars().skip(1).take(kick_player_name.len()-2).collect();

                                if player.player_name.contains(&match_string) || player.player_name == kick_player_name{
                                    kick_player_list.push((player_index, player.player_name.clone(), player.addr));
                                }
                            },
                            _=>{}
                        }
                    }
                }
                if !kick_player_list.is_empty() {
                    for (player_index, player_name, player_addr) in kick_player_list {
                        if player_index != admin_player_index {
                            self.remove_player(player_index);

                            if ban_player{
                                self.ban_list.insert(player_addr.ip());

                                info!("{} ({}) banned {} ({})", admin_player_name, admin_player_index, player_name, player_index);
                                let msg = format!("{} banned by {}",player_name, admin_player_name);
                                self.add_server_chat_message(msg);
                            } else {
                                info!("{} ({}) kicked {} ({})", admin_player_name, admin_player_index, player_name, player_index);
                                let msg = format!("{} kicked by {}",player_name, admin_player_name);
                                self.add_server_chat_message(msg);
                            }
                        } else {
                            if ban_player{
                                let msg = format!("You cannot ban yourself");
                                self.add_directed_server_chat_message(msg,admin_player_index);
                            } else {
                                let msg = format!("You cannot kick yourself");
                                self.add_directed_server_chat_message(msg,admin_player_index);
                            }
                        }
                    }

                } else {
                    match match_mode {
                        0 =>{ // full string
                            let msg = format!("No player names match {}",kick_player_name);
                            self.add_directed_server_chat_message(msg,admin_player_index);
                        },
                        1 =>{ // begins with%
                            let msg = format!("No player names begin with {}",kick_player_name);
                            self.add_directed_server_chat_message(msg,admin_player_index);
                        },
                        2 =>{ // %ends with
                            let msg = format!("No player names end with {}",kick_player_name);
                            self.add_directed_server_chat_message(msg,admin_player_index);
                        },
                        3 =>{ // %contains%
                            let msg = format!("No player names contain {}",kick_player_name);
                            self.add_directed_server_chat_message(msg,admin_player_index);
                        },
                        _=>{}
                    }
                }

            } else{
                self.admin_deny_message(admin_player_index);
                return;
            }
        }

    }

    pub(crate) fn kick_player (& mut self, admin_player_index: usize, kick_player_index: usize, ban_player: bool) {

        if let Some(player) = & self.players[admin_player_index]{
            if player.is_admin {
                let admin_player_name = player.player_name.clone();

                if kick_player_index != admin_player_index {
                    if kick_player_index < self.players.len() {
                        if let Some(kick_player) = & mut self.players[kick_player_index as usize] {
                            let kick_player_name = kick_player.player_name.clone ();
                            let kick_ip = kick_player.addr.ip().clone();
                            self.remove_player(kick_player_index);

                            if ban_player {
                                self.ban_list.insert(kick_ip);

                                info!("{} ({}) banned {} ({})", admin_player_name, admin_player_index, kick_player_name, kick_player_name);
                                let msg = format!("{} banned by {}", kick_player_name, admin_player_name);
                                self.add_server_chat_message(msg);
                            } else {
                                info!("{} ({}) kicked {} ({})", admin_player_name, admin_player_index, kick_player_name, kick_player_name);
                                let msg = format!("{} kicked by {}", kick_player_name, admin_player_name);
                                self.add_server_chat_message(msg);
                            }
                        }
                    }
                } else {
                    if ban_player{
                        let msg = format!("You cannot ban yourself");
                        self.add_directed_server_chat_message(msg,admin_player_index);
                    } else {
                        let msg = format!("You cannot kick yourself");
                        self.add_directed_server_chat_message(msg,admin_player_index);
                    }
                }
            } else {
                self.admin_deny_message(admin_player_index);
                return;
            }
        }

    }

    pub(crate) fn clear_bans (& mut self, player_index: usize) {
        if let Some(player) = & self.players[player_index] {
            if player.is_admin{
                self.ban_list.clear();
                info!("{} ({}) cleared bans", player.player_name, player_index);

                let msg = format!("Bans cleared by {}",player.player_name);
                self.add_server_chat_message(msg);
            } else {
                self.admin_deny_message(player_index);
            }
        }
    }




    pub(crate) fn set_replay (& mut self, player_index: usize, rule:&str) {
        if let Some(player) = & self.players[player_index] {
            if player.is_admin{
                match rule {
                    "on" => {
                        self.config.replays_enabled = true;
                        if self.game.replay_data.len() < 64 * 1024 * 1024 {
                            self.game.replay_data.reserve((64 * 1024 * 1024) - self.game.replay_data.len())
                        }

                        info!("{} ({}) enabled replays",player.player_name, player_index);
                        let msg = format!("Replays enabled by {}", player.player_name);

                        self.add_server_chat_message(msg);
                    },
                    "off" => {
                        self.config.replays_enabled = false;

                        info!("{} ({}) disabled replays",player.player_name, player_index);
                        let msg = format!("Replays disabled by {}", player.player_name);

                        self.add_server_chat_message(msg);
                    }
                    _ => {}
                }
            } else {
                self.admin_deny_message(player_index);
            }
        }
    }


}
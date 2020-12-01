use crate::hqm_server::HQMServer;
use crate::hqm_game::{HQMGameObject, HQMMessage, HQMTeam};
use std::net::SocketAddr;

impl HQMServer {
    fn admin_deny_message (& mut self, player_index: usize) {
        let msg = format!("Please log in before using that command");
        self.add_directed_server_chat_message(msg,player_index);
    }

    pub(crate) fn set_allow_join (& mut self, player_index: usize, allowed: bool) {
        if let Some(player) = & self.players[player_index] {
            if player.is_admin{
                self.allow_join=allowed;

                if allowed {
                    let msg = format!("Joins enabled by {}",player.player_name);
                    self.add_server_chat_message(msg);
                } else {
                    let msg = format!("Joins disabled by {}",player.player_name);
                    self.add_server_chat_message(msg);
                }

            } else {
                self.admin_deny_message(player_index);
            }
        }
    }

    pub(crate) fn mute_player (& mut self, player_index: usize, mute_player: String) {
        if let Some(player) = & self.players[player_index]{
            if player.is_admin {
                let admin_player_name = player.player_name.clone();
                let mut player_found:bool = false;

                for p in self.players.iter_mut() {
                    if let Some(player) = p {
                        if player.player_name == mute_player{
                            player.is_muted=true;
                            player_found=true;
                        }
                    }
                }

                if player_found{
                    let msg = format!("{} muted by {}",mute_player,admin_player_name);
                    self.add_server_chat_message(msg);
                }
            } else {
                self.admin_deny_message(player_index);
            }
        }

    }

    pub(crate) fn unmute_player (& mut self, player_index: usize, mute_player: String) {
        if let Some(player) = & self.players[player_index] {
            if player.is_admin {
                let admin_player_name = player.player_name.clone();
                let mut player_found:bool = false;

                for p in self.players.iter_mut() {
                    if let Some(player) = p {
                        if player.player_name == mute_player{
                            player.is_muted=false;
                            player_found=true;
                        }
                    }
                }

                if player_found{
                    let msg = format!("{} unmuted by {}",mute_player,admin_player_name);
                    self.add_server_chat_message(msg);
                }
            } else {
                self.admin_deny_message(player_index);
            }
        }
    }

    pub(crate) fn mute_chat (& mut self, player_index: usize) {
        if let Some(player) = & self.players[player_index] {
            if player.is_admin{
                self.is_muted=true;

                let msg = format!("Chat muted by {}",player.player_name);
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
                self.add_server_chat_message(msg);
            } else {
                self.admin_deny_message(player_index);
            }
        }
    }

    pub(crate) fn force_player_off_ice (& mut self, player_index: usize, force_player_off_number: u32) {

        let mut admin_player_name = "".to_string();

        if let Some(player) = & self.players[player_index] {
            if player.is_admin {
                admin_player_name = player.player_name.clone();
            } else {
                self.admin_deny_message(player_index);
                return;
            }
        }

        let force_player_index_number = force_player_off_number - 1;

        if (force_player_index_number as usize) < self.players.len() {
            if let Some(force_player) = & mut self.players[force_player_index_number as usize] {

                force_player.team_switch_timer = 500; // 500 ticks, 5 seconds
                if let Some (i) = force_player.skater {
                    self.game.world.objects[i] = HQMGameObject::None;
                    force_player.skater = None;
                    let force_player_name = force_player.player_name.clone();
                    let msg = format!("{} forced off ice by {}",force_player_name,admin_player_name);

                    self.add_global_message(HQMMessage::PlayerUpdate {
                        player_name: force_player_name,
                        team: None,
                        player_index: force_player_index_number as usize,
                        object_index: None,
                        in_server: true
                    },true);

                    self.add_server_chat_message(msg);
                }
            }
        }
    }

    pub(crate) fn set_role (& mut self, player_index: usize, input_position:&str) {
        let input_position = input_position.to_uppercase();
        if self.game.world.rink.allowed_positions.contains(& input_position) {
            if let Some(player) = & mut self.players[player_index] {
                player.faceoff_position = input_position;

                let msg = format!("{} position {}", player.player_name, player.faceoff_position);
                self.add_server_chat_message(msg);

            }
        }
    }

    pub(crate) fn admin_login (& mut self, player_index: usize, password:&str) {
        if let Some(player) = & mut self.players[player_index] {

            if self.config.password == password{
                player.is_admin = true;

                let msg = format!("{} admin", player.player_name);
                self.add_server_chat_message(msg);
            } else {

                let msg = format!("Incorrect password");
                self.add_directed_server_chat_message(msg,player_index);

            }
        }
    }

    pub(crate) fn kick_player (& mut self, admin_player_index: usize, kick_player_name: String, ban_player: bool) {

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

                                let msg = format!("{} banned by {}",player_name, admin_player_name);
                                self.add_server_chat_message(msg);
                            } else {
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

    pub(crate) fn clear_bans (& mut self, player_index: usize) {
        if let Some(player) = & self.players[player_index] {
            if player.is_admin{
                self.ban_list.clear();

                let msg = format!("Bans cleared by {}",player.player_name);
                self.add_server_chat_message(msg);
            } else {
                self.admin_deny_message(player_index);
            }
        }
    }

    pub(crate) fn set_clock (& mut self, input_minutes: u32, input_seconds: u32, player_index: usize) {
        if let Some(player) = & self.players[player_index] {
            if player.is_admin{
                self.game.time = (input_minutes * 60 * 100)+ (input_seconds * 100);

                let msg = format!("Clock set by {}", player.player_name);
                self.add_server_chat_message(msg);
            } else {
                self.admin_deny_message(player_index);
            }
        }

    }

    pub(crate) fn set_score (& mut self, input_team: HQMTeam, input_score: u32, player_index: usize) {
        if let Some(player) = & self.players[player_index] {
            if player.is_admin{
                match input_team {
                    HQMTeam::Red =>{
                        self.game.red_score = input_score;

                        let msg = format!("Red score changed by {}",player.player_name);
                        self.add_server_chat_message(msg);
                    },
                    HQMTeam::Blue =>{
                        self.game.blue_score = input_score;

                        let msg = format!("Blue score changed by {}",player.player_name);
                        self.add_server_chat_message(msg);
                    },
                }
            } else {
                self.admin_deny_message(player_index);
            }
        }
    }

    pub(crate) fn set_period (& mut self, input_period: u32, player_index: usize) {
        if let Some(player) = & self.players[player_index] {
            if player.is_admin{

                self.game.period = input_period;

                let msg = format!("Period set by {}",player.player_name);
                self.add_server_chat_message(msg);

            } else {
                self.admin_deny_message(player_index);
            }
        }
    }

    pub(crate) fn faceoff (& mut self, player_index: usize) {
        if let Some(player) = & self.players[player_index] {
            if player.is_admin{
                self.game.intermission = 5*100;

                let msg = format!("Faceoff initiated by {}",player.player_name);
                self.add_server_chat_message(msg);
            } else {
                self.admin_deny_message(player_index);
            }
        }
    }

    pub(crate) fn reset_game (& mut self, player_index: usize) {
        if let Some(player) = & self.players[player_index] {
            if player.is_admin{
                let msg = format!("Game reset by {}",player.player_name);

                self.new_game();

                self.add_server_chat_message(msg);
            } else {
                self.admin_deny_message(player_index);
            }
        }
    }

    pub(crate) fn pause (& mut self, player_index: usize) {
        if let Some(player) = & self.players[player_index] {
            if player.is_admin{
                self.game.paused=true;

                let msg = format!("Game paused by {}",player.player_name);
                self.add_server_chat_message(msg);
            } else {
                self.admin_deny_message(player_index);
            }
        }
    }

    pub(crate) fn unpause (& mut self, player_index: usize) {
        if let Some(player) = & self.players[player_index] {
            if player.is_admin{
                self.game.paused=false;

                let msg = format!("Game resumed by {}",player.player_name);
                self.add_server_chat_message(msg);
            } else {
                self.admin_deny_message(player_index);
            }
        }
    }
}
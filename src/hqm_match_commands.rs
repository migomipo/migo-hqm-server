use crate::hqm_game::HQMTeam;
use crate::hqm_match_util::{
    HQMIcingConfiguration, HQMMatch, HQMOffsideConfiguration, HQMOffsideLineConfiguration,
    HQMTwoLinePassConfiguration,
};
use crate::hqm_server::{HQMServer, HQMServerPlayerIndex};
use tracing::info;

impl HQMMatch {
    pub fn reset_game(&mut self, server: &mut HQMServer, player_index: HQMServerPlayerIndex) {
        if let Some(player) = server.players.get(player_index) {
            if player.is_admin {
                info!("{} ({}) reset game", player.player_name, player_index);
                let msg = format!("Game reset by {}", player.player_name);

                server.new_game(self.create_game());

                server.messages.add_server_chat_message(msg);
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    pub fn start_game(&mut self, server: &mut HQMServer, player_index: HQMServerPlayerIndex) {
        if let Some(player) = server.players.get(player_index) {
            if player.is_admin {
                if server.game.period == 0 && server.game.time > 1 {
                    info!("{} ({}) started game", player.player_name, player_index);
                    let msg = format!("Game started by {}", player.player_name);
                    self.paused = false;
                    server.game.time = 1;

                    server.messages.add_server_chat_message(msg);
                }
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    pub fn pause(&mut self, server: &mut HQMServer, player_index: HQMServerPlayerIndex) {
        if let Some(player) = server.players.get(player_index) {
            if player.is_admin {
                self.paused = true;
                if self.pause_timer > 0 && self.pause_timer < self.config.time_break {
                    // If we're currently in a break, with very little time left,
                    // we reset the timer
                    self.pause_timer = self.config.time_break;
                }
                info!("{} ({}) paused game", player.player_name, player_index);
                let msg = format!("Game paused by {}", player.player_name);
                server.messages.add_server_chat_message(msg);
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    pub fn unpause(&mut self, server: &mut HQMServer, player_index: HQMServerPlayerIndex) {
        if let Some(player) = server.players.get(player_index) {
            if player.is_admin {
                self.paused = false;
                info!("{} ({}) resumed game", player.player_name, player_index);
                let msg = format!("Game resumed by {}", player.player_name);

                server.messages.add_server_chat_message(msg);
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    pub fn set_clock(
        &mut self,
        server: &mut HQMServer,
        input_time: u32,
        player_index: HQMServerPlayerIndex,
    ) {
        if let Some(player) = server.players.get(player_index) {
            if player.is_admin {
                server.game.time = input_time;

                let input_minutes = input_time / (60 * 100);
                let input_rest = input_time % (60 * 100);
                let input_seconds = input_rest / 100;
                let input_centis = input_time % 100;

                info!(
                    "Clock set to {}:{:02}.{:02} by {} ({})",
                    input_minutes, input_seconds, input_centis, player.player_name, player_index
                );
                let msg = format!("Clock set by {}", player.player_name);
                server.messages.add_server_chat_message(msg);
                self.update_game_over(server);
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    pub fn set_score(
        &mut self,
        server: &mut HQMServer,
        input_team: HQMTeam,
        input_score: u32,
        player_index: HQMServerPlayerIndex,
    ) {
        if let Some(player) = server.players.get(player_index) {
            if player.is_admin {
                match input_team {
                    HQMTeam::Red => {
                        server.game.red_score = input_score;

                        info!(
                            "{} ({}) changed red score to {}",
                            player.player_name, player_index, input_score
                        );
                        let msg = format!("Red score changed by {}", player.player_name);
                        server.messages.add_server_chat_message(msg);
                    }
                    HQMTeam::Blue => {
                        server.game.blue_score = input_score;

                        info!(
                            "{} ({}) changed blue score to {}",
                            player.player_name, player_index, input_score
                        );
                        let msg = format!("Blue score changed by {}", player.player_name);
                        server.messages.add_server_chat_message(msg);
                    }
                }
                self.update_game_over(server);
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    pub fn set_period(
        &mut self,
        server: &mut HQMServer,
        input_period: u32,
        player_index: HQMServerPlayerIndex,
    ) {
        if let Some(player) = server.players.get(player_index) {
            if player.is_admin {
                server.game.period = input_period;

                info!(
                    "{} ({}) set period to {}",
                    player.player_name, player_index, input_period
                );
                let msg = format!("Period set by {}", player.player_name);
                server.messages.add_server_chat_message(msg);
                self.update_game_over(server);
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    pub fn set_period_num(
        &mut self,
        server: &mut HQMServer,
        input_period: u32,
        player_index: HQMServerPlayerIndex,
    ) {
        if let Some(player) = server.players.get(player_index) {
            if player.is_admin {
                self.config.periods = input_period;

                info!(
                    "{} ({}) set number of periods to {}",
                    player.player_name, player_index, input_period
                );
                let msg = format!(
                    "Number of periods set to {} by {}",
                    input_period, player.player_name
                );
                server.messages.add_server_chat_message(msg);
                self.update_game_over(server);
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    pub fn set_icing_rule(
        &mut self,
        server: &mut HQMServer,
        player_index: HQMServerPlayerIndex,
        rule: &str,
    ) {
        if let Some(player) = server.players.get(player_index) {
            if player.is_admin {
                match rule {
                    "on" | "touch" => {
                        self.config.icing = HQMIcingConfiguration::Touch;
                        info!(
                            "{} ({}) enabled touch icing",
                            player.player_name, player_index
                        );
                        let msg = format!("Touch icing enabled by {}", player.player_name);

                        server.messages.add_server_chat_message(msg);
                    }
                    "notouch" => {
                        self.config.icing = HQMIcingConfiguration::NoTouch;
                        info!(
                            "{} ({}) enabled no-touch icing",
                            player.player_name, player_index
                        );
                        let msg = format!("No-touch icing enabled by {}", player.player_name);

                        server.messages.add_server_chat_message(msg);
                    }
                    "off" => {
                        self.config.icing = HQMIcingConfiguration::Off;
                        info!("{} ({}) disabled icing", player.player_name, player_index);
                        let msg = format!("Icing disabled by {}", player.player_name);

                        server.messages.add_server_chat_message(msg);
                    }
                    _ => {}
                }
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    pub fn set_offside_line(
        &mut self,
        server: &mut HQMServer,
        player_index: HQMServerPlayerIndex,
        rule: &str,
    ) {
        if let Some(player) = server.players.get(player_index) {
            if player.is_admin {
                match rule {
                    "blue" => {
                        self.config.offside_line = HQMOffsideLineConfiguration::OffensiveBlue;
                        info!(
                            "{} ({}) set blue line as offside line",
                            player.player_name, player_index
                        );
                        let msg =
                            format!("Blue line set as offside line by {}", player.player_name);

                        server.messages.add_server_chat_message(msg);
                    }
                    "center" => {
                        self.config.offside_line = HQMOffsideLineConfiguration::Center;
                        info!(
                            "{} ({}) set center line as offside line",
                            player.player_name, player_index
                        );
                        let msg =
                            format!("Center line set as offside line by {}", player.player_name);

                        server.messages.add_server_chat_message(msg);
                    }
                    _ => {}
                }
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    pub fn set_twoline_pass(
        &mut self,
        server: &mut HQMServer,
        player_index: HQMServerPlayerIndex,
        rule: &str,
    ) {
        if let Some(player) = server.players.get(player_index) {
            if player.is_admin {
                match rule {
                    "off" => {
                        self.config.twoline_pass = HQMTwoLinePassConfiguration::Off;
                        info!(
                            "{} ({}) disabled two-line pass rule",
                            player.player_name, player_index
                        );
                        let msg = format!("Two-line pass rule disabled by {}", player.player_name);

                        server.messages.add_server_chat_message(msg);
                    }
                    "on" => {
                        self.config.twoline_pass = HQMTwoLinePassConfiguration::On;
                        info!(
                            "{} ({}) enabled regular two-line pass rule",
                            player.player_name, player_index
                        );
                        let msg = format!(
                            "Regular two-line pass rule enabled by {}",
                            player.player_name
                        );

                        server.messages.add_server_chat_message(msg);
                    }
                    "forward" => {
                        self.config.twoline_pass = HQMTwoLinePassConfiguration::Forward;
                        info!(
                            "{} ({}) enabled forward two-line pass rule",
                            player.player_name, player_index
                        );
                        let msg = format!(
                            "Forward two-line pass rule enabled by {}",
                            player.player_name
                        );

                        server.messages.add_server_chat_message(msg);
                    }
                    "double" | "both" => {
                        self.config.twoline_pass = HQMTwoLinePassConfiguration::Double;
                        info!(
                            "{} ({}) enabled regular and forward two-line pass rule",
                            player.player_name, player_index
                        );
                        let msg = format!(
                            "Regular and forward two-line pass rule enabled by {}",
                            player.player_name
                        );

                        server.messages.add_server_chat_message(msg);
                    }
                    "blue" | "three" | "threeline" => {
                        self.config.twoline_pass = HQMTwoLinePassConfiguration::ThreeLine;
                        info!(
                            "{} ({}) enabled three-line pass rule",
                            player.player_name, player_index
                        );
                        let msg = format!("Three-line pass rule enabled by {}", player.player_name);

                        server.messages.add_server_chat_message(msg);
                    }
                    _ => {}
                }
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    pub fn set_offside_rule(
        &mut self,
        server: &mut HQMServer,
        player_index: HQMServerPlayerIndex,
        rule: &str,
    ) {
        if let Some(player) = server.players.get(player_index) {
            if player.is_admin {
                match rule {
                    "on" | "delayed" => {
                        self.config.offside = HQMOffsideConfiguration::Delayed;
                        info!("{} ({}) enabled offside", player.player_name, player_index);
                        let msg = format!("Offside enabled by {}", player.player_name);

                        server.messages.add_server_chat_message(msg);
                    }
                    "imm" | "immediate" => {
                        self.config.offside = HQMOffsideConfiguration::Immediate;
                        info!(
                            "{} ({}) enabled immediate offside",
                            player.player_name, player_index
                        );
                        let msg = format!("Immediate offside enabled by {}", player.player_name);

                        server.messages.add_server_chat_message(msg);
                    }
                    "off" => {
                        self.config.offside = HQMOffsideConfiguration::Off;
                        info!("{} ({}) disabled offside", player.player_name, player_index);
                        let msg = format!("Offside disabled by {}", player.player_name);

                        server.messages.add_server_chat_message(msg);
                    }
                    _ => {}
                }
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    pub fn set_goal_replay(
        &mut self,
        server: &mut HQMServer,
        player_index: HQMServerPlayerIndex,
        setting: &str,
    ) {
        if let Some(player) = server.players.get(player_index) {
            if player.is_admin {
                match setting {
                    "on" => {
                        self.config.goal_replay = true;
                        let msg = format!("Goal replays enabled by {}", player.player_name);
                        server.messages.add_server_chat_message(msg);
                    }
                    "off" => {
                        self.config.goal_replay = false;
                        let msg = format!("Goal replays disabled by {}", player.player_name);
                        server.messages.add_server_chat_message(msg);
                    }
                    _ => {}
                }
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    pub fn set_first_to_rule(
        &mut self,
        server: &mut HQMServer,
        player_index: HQMServerPlayerIndex,
        num: &str,
    ) {
        if let Some(player) = server.players.get(player_index) {
            if player.is_admin {
                let num = if num == "off" {
                    Some(0)
                } else {
                    num.parse::<u32>().ok()
                };
                if let Some(new_num) = num {
                    self.config.first_to = new_num;

                    if new_num > 0 {
                        info!(
                            "{} ({}) set first-to-goals rule to {} goals",
                            player.player_name, player_index, new_num
                        );
                        let msg = format!(
                            "First-to-goals rule set to {} goals by {}",
                            new_num, player.player_name
                        );
                        server.messages.add_server_chat_message(msg);
                    } else {
                        info!(
                            "{} ({}) disabled first-to-goals rule",
                            player.player_name, player_index
                        );
                        let msg = format!("First-to-goals rule disabled by {}", player.player_name);
                        server.messages.add_server_chat_message(msg);
                    }
                }
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    pub fn set_mercy_rule(
        &mut self,
        server: &mut HQMServer,
        player_index: HQMServerPlayerIndex,
        num: &str,
    ) {
        if let Some(player) = server.players.get(player_index) {
            if player.is_admin {
                let num = if num == "off" {
                    Some(0)
                } else {
                    num.parse::<u32>().ok()
                };
                if let Some(new_num) = num {
                    self.config.mercy = new_num;

                    if new_num > 0 {
                        info!(
                            "{} ({}) set mercy rule to {} goals",
                            player.player_name, player_index, new_num
                        );
                        let msg = format!(
                            "Mercy rule set to {} goals by {}",
                            new_num, player.player_name
                        );
                        server.messages.add_server_chat_message(msg);
                    } else {
                        info!(
                            "{} ({}) disabled mercy rule",
                            player.player_name, player_index
                        );
                        let msg = format!("Mercy rule disabled by {}", player.player_name);
                        server.messages.add_server_chat_message(msg);
                    }
                }
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    pub fn faceoff(&mut self, server: &mut HQMServer, player_index: HQMServerPlayerIndex) {
        if !server.game.game_over {
            if let Some(player) = server.players.get(player_index) {
                if player.is_admin {
                    self.pause_timer = 5 * 100;
                    self.paused = false; // Unpause if it's paused as well

                    let msg = format!("Faceoff initiated by {}", player.player_name);
                    info!(
                        "{} ({}) initiated faceoff",
                        player.player_name, player_index
                    );
                    server.messages.add_server_chat_message(msg);
                } else {
                    server.admin_deny_message(player_index);
                }
            }
        }
    }

    pub fn set_preferred_faceoff_position(
        &mut self,
        server: &mut HQMServer,
        player_index: HQMServerPlayerIndex,
        input_position: &str,
    ) {
        let input_position = input_position.to_uppercase();
        if server
            .game
            .world
            .rink
            .allowed_positions
            .contains(&input_position)
        {
            if let Some(player) = server.players.get(player_index) {
                info!(
                    "{} ({}) set position {}",
                    player.player_name, player_index, input_position
                );
                let msg = format!("{} position {}", player.player_name, input_position);

                self.preferred_positions
                    .insert(player_index, input_position);
                server.messages.add_server_chat_message(msg);
            }
        }
    }

    pub fn msg_rules(&self, server: &mut HQMServer, receiver_index: HQMServerPlayerIndex) {
        let offside_str = match self.config.offside {
            HQMOffsideConfiguration::Off => "Offside disabled",
            HQMOffsideConfiguration::Delayed => "Offside enabled",
            HQMOffsideConfiguration::Immediate => "Immediate offside enabled",
        };
        let offside_line_str = if self.config.offside != HQMOffsideConfiguration::Off
            && self.config.offside_line == HQMOffsideLineConfiguration::Center
        {
            " (center line)"
        } else {
            ""
        };
        let icing_str = match self.config.icing {
            HQMIcingConfiguration::Off => "Icing disabled",
            HQMIcingConfiguration::Touch => "Icing enabled",
            HQMIcingConfiguration::NoTouch => "No-touch icing enabled",
        };

        let msg = format!("{}{}, {}", offside_str, offside_line_str, icing_str);
        server
            .messages
            .add_directed_server_chat_message(msg, receiver_index);
        let twoline_str = match self.config.twoline_pass {
            HQMTwoLinePassConfiguration::Off => "",
            HQMTwoLinePassConfiguration::On => "Two-line pass rule enabled",
            HQMTwoLinePassConfiguration::Forward => "Forward two-line pass rule enabled",
            HQMTwoLinePassConfiguration::Double => "Forward and regular two-line pass rule enabled",
            HQMTwoLinePassConfiguration::ThreeLine => "Three-line pass rule enabled",
        };
        if !twoline_str.is_empty() {
            server
                .messages
                .add_directed_server_chat_message_str(twoline_str, receiver_index);
        }

        if self.config.mercy > 0 {
            let msg = format!("Mercy rule when team leads by {} goals", self.config.mercy);
            server
                .messages
                .add_directed_server_chat_message(msg, receiver_index);
        }
        if self.config.first_to > 0 {
            let msg = format!("Game ends when team scores {} goals", self.config.first_to);
            server
                .messages
                .add_directed_server_chat_message(msg, receiver_index);
        }
    }
}

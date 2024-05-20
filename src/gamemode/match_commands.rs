use crate::game::PlayerId;
use crate::game::Team;
use crate::gamemode::ServerMut;

use crate::gamemode::match_util::{
    IcingConfiguration, Match, OffsideConfiguration, OffsideLineConfiguration,
    TwoLinePassConfiguration, ALLOWED_POSITIONS,
};
use tracing::info;

impl Match {
    pub fn reset_game(&mut self, mut server: ServerMut, player_id: PlayerId) {
        if let Some(player) = server
            .state_mut()
            .players_mut()
            .check_admin_or_deny(player_id)
        {
            let name = player.name();
            info!("{} ({}) reset game", name, player_id);
            let msg = format!("Game reset by {}", name);

            server.new_game(self.get_initial_game_values());

            server.state_mut().add_server_chat_message(msg);
        }
    }

    pub fn start_game(&mut self, mut server: ServerMut, player_id: PlayerId) {
        if let Some(player) = server
            .state_mut()
            .players_mut()
            .check_admin_or_deny(player_id)
        {
            let name = player.name();
            let values = server.scoreboard_mut();
            if values.period == 0 && values.time > 1 {
                info!("{} ({}) started game", name, player_id);
                let msg = format!("Game started by {}", name);
                self.paused = false;
                values.time = 1;

                server.state_mut().add_server_chat_message(msg);
            }
        }
    }

    pub fn pause(&mut self, mut server: ServerMut, player_id: PlayerId) {
        if let Some(player) = server
            .state_mut()
            .players_mut()
            .check_admin_or_deny(player_id)
        {
            self.paused = true;
            if self.pause_timer > 0 && self.pause_timer < self.config.time_break {
                // If we're currently in a break, with very little time left,
                // we reset the timer
                self.pause_timer = self.config.time_break;
            }
            let name = player.name();
            info!("{} ({}) paused game", name, player_id);
            let msg = format!("Game paused by {}", name);
            server.state_mut().add_server_chat_message(msg);
        }
    }

    pub fn unpause(&mut self, mut server: ServerMut, player_id: PlayerId) {
        if let Some(player) = server
            .state_mut()
            .players_mut()
            .check_admin_or_deny(player_id)
        {
            self.paused = false;
            let name = player.name();
            info!("{} ({}) resumed game", name, player_id);
            let msg = format!("Game resumed by {}", name);

            server.state_mut().add_server_chat_message(msg);
        }
    }

    pub fn set_clock(&mut self, mut server: ServerMut, input_time: u32, player_id: PlayerId) {
        if let Some(player) = server
            .state_mut()
            .players_mut()
            .check_admin_or_deny(player_id)
        {
            let name = player.name();
            server.scoreboard_mut().time = input_time;

            let input_minutes = input_time / (60 * 100);
            let input_rest = input_time % (60 * 100);
            let input_seconds = input_rest / 100;
            let input_centis = input_time % 100;

            info!(
                "Clock set to {}:{:02}.{:02} by {} ({})",
                input_minutes, input_seconds, input_centis, name, player_id
            );
            let msg = format!("Clock set by {}", name);
            server.state_mut().add_server_chat_message(msg);
            self.update_game_over(server);
        }
    }

    pub fn set_score(
        &mut self,
        mut server: ServerMut,
        input_team: Team,
        input_score: u32,
        player_id: PlayerId,
    ) {
        if let Some(player) = server
            .state_mut()
            .players_mut()
            .check_admin_or_deny(player_id)
        {
            let name = player.name();
            match input_team {
                Team::Red => {
                    server.scoreboard_mut().red_score = input_score;

                    info!(
                        "{} ({}) changed red score to {}",
                        name, player_id, input_score
                    );
                    let msg = format!("Red score changed by {}", name);
                    server.state_mut().add_server_chat_message(msg);
                }
                Team::Blue => {
                    server.scoreboard_mut().blue_score = input_score;

                    info!(
                        "{} ({}) changed blue score to {}",
                        name, player_id, input_score
                    );
                    let msg = format!("Blue score changed by {}", name);
                    server.state_mut().add_server_chat_message(msg);
                }
            }
            self.update_game_over(server);
        }
    }

    pub fn set_period(&mut self, mut server: ServerMut, input_period: u32, player_id: PlayerId) {
        if let Some(player) = server
            .state_mut()
            .players_mut()
            .check_admin_or_deny(player_id)
        {
            let name = player.name();
            server.scoreboard_mut().period = input_period;

            info!("{} ({}) set period to {}", name, player_id, input_period);
            let msg = format!("Period set by {}", name);
            server.state_mut().add_server_chat_message(msg);
            self.update_game_over(server);
        }
    }

    pub fn set_period_num(
        &mut self,
        mut server: ServerMut,
        input_period: u32,
        player_id: PlayerId,
    ) {
        if let Some(player) = server
            .state_mut()
            .players_mut()
            .check_admin_or_deny(player_id)
        {
            self.config.periods = input_period;
            let name = player.name();

            info!(
                "{} ({}) set number of periods to {}",
                name, player_id, input_period
            );
            let msg = format!("Number of periods set to {} by {}", input_period, name);
            server.state_mut().add_server_chat_message(msg);
            self.update_game_over(server);
        }
    }

    pub fn set_icing_rule(&mut self, mut server: ServerMut, player_id: PlayerId, rule: &str) {
        if let Some(player) = server
            .state_mut()
            .players_mut()
            .check_admin_or_deny(player_id)
        {
            let name = player.name();

            match rule {
                "on" | "touch" => {
                    self.config.icing = IcingConfiguration::Touch;
                    info!("{} ({}) enabled touch icing", name, player_id);
                    let msg = format!("Touch icing enabled by {}", name);

                    server.state_mut().add_server_chat_message(msg);
                }
                "notouch" => {
                    self.config.icing = IcingConfiguration::NoTouch;
                    info!("{} ({}) enabled no-touch icing", name, player_id);
                    let msg = format!("No-touch icing enabled by {}", name);

                    server.state_mut().add_server_chat_message(msg);
                }
                "off" => {
                    self.config.icing = IcingConfiguration::Off;
                    info!("{} ({}) disabled icing", name, player_id);
                    let msg = format!("Icing disabled by {}", name);

                    server.state_mut().add_server_chat_message(msg);
                }
                _ => {}
            }
        }
    }

    pub fn set_offside_line(&mut self, mut server: ServerMut, player_id: PlayerId, rule: &str) {
        if let Some(player) = server
            .state_mut()
            .players_mut()
            .check_admin_or_deny(player_id)
        {
            let name = player.name();

            match rule {
                "blue" => {
                    self.config.offside_line = OffsideLineConfiguration::OffensiveBlue;
                    info!("{} ({}) set blue line as offside line", name, player_id);
                    let msg = format!("Blue line set as offside line by {}", name);

                    server.state_mut().add_server_chat_message(msg);
                }
                "center" => {
                    self.config.offside_line = OffsideLineConfiguration::Center;
                    info!("{} ({}) set center line as offside line", name, player_id);
                    let msg = format!("Center line set as offside line by {}", name);

                    server.state_mut().add_server_chat_message(msg);
                }
                _ => {}
            }
        }
    }

    pub fn set_twoline_pass(&mut self, mut server: ServerMut, player_id: PlayerId, rule: &str) {
        if let Some(player) = server
            .state_mut()
            .players_mut()
            .check_admin_or_deny(player_id)
        {
            match rule {
                "off" => {
                    self.config.twoline_pass = TwoLinePassConfiguration::Off;
                    let name = player.name();
                    info!("{} ({}) disabled two-line pass rule", name, player_id);
                    let msg = format!("Two-line pass rule disabled by {}", name);

                    server.state_mut().add_server_chat_message(msg);
                }
                "on" => {
                    self.config.twoline_pass = TwoLinePassConfiguration::On;
                    let name = player.name();

                    info!(
                        "{} ({}) enabled regular two-line pass rule",
                        name, player_id
                    );
                    let msg = format!("Regular two-line pass rule enabled by {}", name);

                    server.state_mut().add_server_chat_message(msg);
                }
                "forward" => {
                    self.config.twoline_pass = TwoLinePassConfiguration::Forward;
                    let name = player.name();

                    info!(
                        "{} ({}) enabled forward two-line pass rule",
                        name, player_id
                    );
                    let msg = format!("Forward two-line pass rule enabled by {}", name);

                    server.state_mut().add_server_chat_message(msg);
                }
                "double" | "both" => {
                    self.config.twoline_pass = TwoLinePassConfiguration::Double;
                    let name = player.name();

                    info!(
                        "{} ({}) enabled regular and forward two-line pass rule",
                        name, player_id
                    );
                    let msg = format!("Regular and forward two-line pass rule enabled by {}", name);

                    server.state_mut().add_server_chat_message(msg);
                }
                "blue" | "three" | "threeline" => {
                    self.config.twoline_pass = TwoLinePassConfiguration::ThreeLine;
                    let name = player.name();

                    info!("{} ({}) enabled three-line pass rule", name, player_id);
                    let msg = format!("Three-line pass rule enabled by {}", name);

                    server.state_mut().add_server_chat_message(msg);
                }
                _ => {}
            }
        }
    }

    pub fn set_offside_rule(&mut self, mut server: ServerMut, player_id: PlayerId, rule: &str) {
        if let Some(player) = server
            .state_mut()
            .players_mut()
            .check_admin_or_deny(player_id)
        {
            match rule {
                "on" | "delayed" => {
                    self.config.offside = OffsideConfiguration::Delayed;
                    let name = player.name();
                    info!("{} ({}) enabled offside", name, player_id);
                    let msg = format!("Offside enabled by {}", name);

                    server.state_mut().add_server_chat_message(msg);
                }
                "imm" | "immediate" => {
                    self.config.offside = OffsideConfiguration::Immediate;

                    let name = player.name();
                    info!("{} ({}) enabled immediate offside", name, player_id);
                    let msg = format!("Immediate offside enabled by {}", name);

                    server.state_mut().add_server_chat_message(msg);
                }
                "off" => {
                    self.config.offside = OffsideConfiguration::Off;

                    let name = player.name();
                    info!("{} ({}) disabled offside", name, player_id);
                    let msg = format!("Offside disabled by {}", name);

                    server.state_mut().add_server_chat_message(msg);
                }
                _ => {}
            }
        }
    }

    pub fn set_goal_replay(&mut self, mut server: ServerMut, player_id: PlayerId, setting: &str) {
        if let Some(player) = server
            .state_mut()
            .players_mut()
            .check_admin_or_deny(player_id)
        {
            match setting {
                "on" => {
                    self.config.goal_replay = true;

                    let name = player.name();
                    let msg = format!("Goal replays enabled by {}", name);
                    server.state_mut().add_server_chat_message(msg);
                }
                "off" => {
                    self.config.goal_replay = false;

                    let name = player.name();
                    let msg = format!("Goal replays disabled by {}", name);
                    server.state_mut().add_server_chat_message(msg);
                }
                _ => {}
            }
        }
    }

    pub fn set_first_to_rule(&mut self, mut server: ServerMut, player_id: PlayerId, num: &str) {
        if let Some(player) = server
            .state_mut()
            .players_mut()
            .check_admin_or_deny(player_id)
        {
            let num = if num == "off" {
                Some(0)
            } else {
                num.parse::<u32>().ok()
            };
            if let Some(new_num) = num {
                self.config.first_to = new_num;
                let name = player.name();

                if new_num > 0 {
                    info!(
                        "{} ({}) set first-to-goals rule to {} goals",
                        name, player_id, new_num
                    );
                    let msg = format!("First-to-goals rule set to {} goals by {}", new_num, name);
                    server.state_mut().add_server_chat_message(msg);
                } else {
                    info!("{} ({}) disabled first-to-goals rule", name, player_id);
                    let msg = format!("First-to-goals rule disabled by {}", name);
                    server.state_mut().add_server_chat_message(msg);
                }
            }
        }
    }

    pub fn set_mercy_rule(&mut self, mut server: ServerMut, player_id: PlayerId, num: &str) {
        if let Some(player) = server
            .state_mut()
            .players_mut()
            .check_admin_or_deny(player_id)
        {
            let num = if num == "off" {
                Some(0)
            } else {
                num.parse::<u32>().ok()
            };
            if let Some(new_num) = num {
                self.config.mercy = new_num;
                let name = player.name();

                if new_num > 0 {
                    info!(
                        "{} ({}) set mercy rule to {} goals",
                        name, player_id, new_num
                    );
                    let msg = format!("Mercy rule set to {} goals by {}", new_num, name);
                    server.state_mut().add_server_chat_message(msg);
                } else {
                    info!("{} ({}) disabled mercy rule", name, player_id);
                    let msg = format!("Mercy rule disabled by {}", name);
                    server.state_mut().add_server_chat_message(msg);
                }
            }
        }
    }

    pub fn faceoff(&mut self, mut server: ServerMut, player_id: PlayerId) {
        if !server.scoreboard().game_over {
            if let Some(player) = server
                .state_mut()
                .players_mut()
                .check_admin_or_deny(player_id)
            {
                self.pause_timer = 5 * 100;
                self.paused = false; // Unpause if it's paused as well

                let name = player.name();
                let msg = format!("Faceoff initiated by {}", name);
                info!("{} ({}) initiated faceoff", name, player_id);
                server.state_mut().add_server_chat_message(msg);
            }
        }
    }

    pub fn set_preferred_faceoff_position(
        &mut self,
        mut server: ServerMut,
        player_id: PlayerId,
        input_position: &str,
    ) {
        let input_position = input_position.to_uppercase();
        if let Some(position) = ALLOWED_POSITIONS
            .into_iter()
            .find(|x| x.eq_ignore_ascii_case(input_position.as_str()))
        {
            if let Some(player) = server.state().players().get_by_id(player_id) {
                let name = player.name();

                info!("{} ({}) set position {}", name, player_id, position);
                let msg = format!("{} position {}", name, position);

                self.preferred_positions.insert(player_id, position);
                server.state_mut().add_server_chat_message(msg);
            }
        }
    }

    pub fn msg_rules(&self, mut server: ServerMut, receiver_id: PlayerId) {
        let offside_str = match self.config.offside {
            OffsideConfiguration::Off => "Offside disabled",
            OffsideConfiguration::Delayed => "Offside enabled",
            OffsideConfiguration::Immediate => "Immediate offside enabled",
        };
        let offside_line_str = if self.config.offside != OffsideConfiguration::Off
            && self.config.offside_line == OffsideLineConfiguration::Center
        {
            " (center line)"
        } else {
            ""
        };
        let icing_str = match self.config.icing {
            IcingConfiguration::Off => "Icing disabled",
            IcingConfiguration::Touch => "Icing enabled",
            IcingConfiguration::NoTouch => "No-touch icing enabled",
        };

        let msg = format!("{}{}, {}", offside_str, offside_line_str, icing_str);
        server
            .state_mut()
            .add_directed_server_chat_message(msg, receiver_id);
        let twoline_str = match self.config.twoline_pass {
            TwoLinePassConfiguration::Off => "",
            TwoLinePassConfiguration::On => "Two-line pass rule enabled",
            TwoLinePassConfiguration::Forward => "Forward two-line pass rule enabled",
            TwoLinePassConfiguration::Double => "Forward and regular two-line pass rule enabled",
            TwoLinePassConfiguration::ThreeLine => "Three-line pass rule enabled",
        };
        if !twoline_str.is_empty() {
            server
                .state_mut()
                .add_directed_server_chat_message(twoline_str, receiver_id);
        }

        if self.config.mercy > 0 {
            let msg = format!("Mercy rule when team leads by {} goals", self.config.mercy);
            server
                .state_mut()
                .add_directed_server_chat_message(msg, receiver_id);
        }
        if self.config.first_to > 0 {
            let msg = format!("Game ends when team scores {} goals", self.config.first_to);
            server
                .state_mut()
                .add_directed_server_chat_message(msg, receiver_id);
        }
    }

    pub fn set_spawn_offset(&mut self, mut server: ServerMut, player_id: PlayerId, rule: f32) {
        if let Some(player) = server
            .state_mut()
            .players_mut()
            .check_admin_or_deny(player_id)
        {
            self.config.spawn_point_offset = rule;

            let name = player.name();
            let msg = format!("Spawn point offset changed by {} to {}", name, rule);
            info!(
                "{} ({}) changed spawn point offset parameter to {}",
                name, player_id, rule
            );
            server.state_mut().add_server_chat_message(msg);
        }
    }

    pub fn set_spawn_player_altitude(
        &mut self,
        mut server: ServerMut,
        player_id: PlayerId,
        rule: f32,
    ) {
        if let Some(player) = server
            .state_mut()
            .players_mut()
            .check_admin_or_deny(player_id)
        {
            self.config.spawn_player_altitude = rule;
            let name = player.name();

            let msg = format!("Spawn player altitude changed by {} to {}", name, rule);
            info!(
                "{} ({}) changed spawn player altitude parameter to {}",
                name, player_id, rule
            );
            server.state_mut().add_server_chat_message(msg);
        }
    }

    pub fn set_spawn_puck_altitude(
        &mut self,
        mut server: ServerMut,
        player_id: PlayerId,
        rule: f32,
    ) {
        if let Some(player) = server
            .state_mut()
            .players_mut()
            .check_admin_or_deny(player_id)
        {
            self.config.spawn_puck_altitude = rule;
            let name = player.name();

            let msg = format!("Spawn puck altitude changed by {} to {}", name, rule);
            info!(
                "{} ({}) changed spawn puck altitude parameter to {}",
                name, player_id, rule
            );
            server.state_mut().add_server_chat_message(msg);
        }
    }

    pub fn set_spawn_keep_stick(
        &mut self,
        mut server: ServerMut,
        player_id: PlayerId,
        setting: &str,
    ) {
        if let Some(player) = server
            .state_mut()
            .players_mut()
            .check_admin_or_deny(player_id)
        {
            let name = player.name();
            let v = match setting {
                "on" | "true" => Some(true),
                "off" | "false" => Some(false),
                _ => None,
            };
            if let Some(v) = v {
                self.config.spawn_keep_stick_position = v;

                let msg = format!("Spawn stick position keeping changed by {} to {}", name, v);
                info!(
                    "{} ({}) changed spawn stick position keeping parameter to {}",
                    name, player_id, v
                );
                server.state_mut().add_server_chat_message(msg);
            }
        }
    }
}

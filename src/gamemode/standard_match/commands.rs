//! Runtime administration commands for the standard-match game mode.

use crate::game::PlayerId;
use crate::gamemode::GameContext;

use super::{
    IcingConfiguration, OffsideConfiguration, OffsideLineConfiguration, StandardMatchGameMode,
    StandardMatchState, TwoLinePassConfiguration,
};

impl StandardMatchGameMode {
    pub(super) fn handle_set(&mut self, ctx: &mut GameContext, arg: &str, player: PlayerId) {
        let Some(admin_name) = ctx.require_admin(player) else {
            return;
        };
        let old_config = self.config;
        let old_team_max = self.teams.team_max;
        let old_state_values = match &self.state {
            StandardMatchState::Game(game) => (
                game.game_state.score,
                game.game_state.period,
                game.game_state.time,
            ),
            StandardMatchState::WaitingForGame(warmup) => (warmup.score, 0, warmup.time),
        };
        let mut parts = arg.split_whitespace();
        let Some(setting) = parts.next() else { return };
        let Some(value) = parts.next() else { return };
        if matches!(setting, "period" | "clock")
            && matches!(self.state, StandardMatchState::WaitingForGame(_))
        {
            ctx.messages
                .add_directed_server_chat_message("No game is currently ongoing", player);
            return;
        }
        match setting {
            "redscore" | "bluescore" => {
                if let Ok(score) = value.parse() {
                    match &mut self.state {
                        StandardMatchState::WaitingForGame(warmup) => {
                            if setting == "redscore" {
                                warmup.score.0 = score
                            } else {
                                warmup.score.1 = score
                            }
                        }
                        StandardMatchState::Game(game) => {
                            if setting == "redscore" {
                                game.game_state.score.0 = score
                            } else {
                                game.game_state.score.1 = score
                            }
                        }
                    }
                }
            }
            "period" => {
                if let Ok(period) = value.parse() {
                    if let StandardMatchState::Game(game) = &mut self.state {
                        game.game_state.period = period;
                    }
                }
            }
            "periodnum" => {
                if let Ok(periods) = value.parse() {
                    self.set_config(|config| config.periods = periods);
                }
            }
            "clock" => {
                if let Some(time) = parse_clock(value) {
                    if let StandardMatchState::Game(game) = &mut self.state {
                        game.game_state.time = time;
                    }
                }
            }
            "icing" => match value {
                "on" | "touch" => self.set_config(|c| c.icing = IcingConfiguration::Touch),
                "notouch" => self.set_config(|c| c.icing = IcingConfiguration::NoTouch),
                "off" => self.set_config(|c| c.icing = IcingConfiguration::Off),
                _ => {}
            },
            "offside" => match value {
                "on" | "delayed" => self.set_config(|c| c.offside = OffsideConfiguration::Delayed),
                "imm" | "immediate" => {
                    self.set_config(|c| c.offside = OffsideConfiguration::Immediate)
                }
                "off" => self.set_config(|c| c.offside = OffsideConfiguration::Off),
                _ => {}
            },
            "twolinepass" => match value {
                "off" => self.set_config(|c| c.twoline_pass = TwoLinePassConfiguration::Off),
                "on" => self.set_config(|c| c.twoline_pass = TwoLinePassConfiguration::On),
                "forward" => {
                    self.set_config(|c| c.twoline_pass = TwoLinePassConfiguration::Forward)
                }
                "double" | "both" => {
                    self.set_config(|c| c.twoline_pass = TwoLinePassConfiguration::Double)
                }
                "blue" | "three" | "threeline" => {
                    self.set_config(|c| c.twoline_pass = TwoLinePassConfiguration::ThreeLine)
                }
                _ => {}
            },
            "offsideline" => match value {
                "blue" => {
                    self.set_config(|c| c.offside_line = OffsideLineConfiguration::OffensiveBlue)
                }
                "center" => self.set_config(|c| c.offside_line = OffsideLineConfiguration::Center),
                _ => {}
            },
            "mercy" => {
                if let Ok(goals) = value.parse() {
                    self.set_config(|c| c.mercy = goals);
                }
            }
            "first" => {
                if let Ok(goals) = value.parse() {
                    self.set_config(|c| c.first_to = goals);
                }
            }
            "goalreplay" => match value {
                "on" => self.set_config(|c| c.goal_replay = true),
                "off" => self.set_config(|c| c.goal_replay = false),
                _ => {}
            },
            "spawnoffset" => {
                if let Ok(offset) = value.parse() {
                    self.set_config(|c| c.spawn_point_offset = offset);
                }
            }
            "spawnplayeraltitude" => {
                if let Ok(altitude) = value.parse() {
                    self.set_config(|c| c.spawn_player_altitude = altitude);
                }
            }
            "spawnpuckaltitude" => {
                if let Ok(altitude) = value.parse() {
                    self.set_config(|c| c.spawn_puck_altitude = altitude);
                }
            }
            "spawnplayerkeepstick" => match value {
                "on" | "true" => self.set_config(|c| c.spawn_keep_stick_position = true),
                "off" | "false" => self.set_config(|c| c.spawn_keep_stick_position = false),
                _ => {}
            },
            "teamsize" => {
                if let Ok(size) = value.parse() {
                    if (1..=15).contains(&size) {
                        self.teams.team_max = size;
                    }
                }
            }
            _ => {}
        }
        let new_state_values = match &self.state {
            StandardMatchState::Game(game) => (
                game.game_state.score,
                game.game_state.period,
                game.game_state.time,
            ),
            StandardMatchState::WaitingForGame(warmup) => (warmup.score, 0, warmup.time),
        };
        if self.config != old_config
            || self.teams.team_max != old_team_max
            || new_state_values != old_state_values
        {
            let message = match (setting, value) {
                ("redscore", _) => format!("Red score changed to {value}"),
                ("bluescore", _) => format!("Blue score changed to {value}"),
                ("period", _) => format!("Period set to {value}"),
                ("periodnum", _) => format!("Number of periods set to {value}"),
                ("clock", _) => format!("Clock set to {value}"),
                ("icing", "on" | "touch") => "Touch icing enabled".to_owned(),
                ("icing", "notouch") => "No-touch icing enabled".to_owned(),
                ("icing", "off") => "Icing disabled".to_owned(),
                ("offside", "on" | "delayed") => "Offside enabled".to_owned(),
                ("offside", "imm" | "immediate") => "Immediate offside enabled".to_owned(),
                ("offside", "off") => "Offside disabled".to_owned(),
                ("twolinepass", "off") => "Two-line pass rule disabled".to_owned(),
                ("twolinepass", "on") => "Regular two-line pass rule enabled".to_owned(),
                ("twolinepass", "forward") => "Forward two-line pass rule enabled".to_owned(),
                ("twolinepass", "double" | "both") => {
                    "Regular and forward two-line pass rules enabled".to_owned()
                }
                ("twolinepass", "blue" | "three" | "threeline") => {
                    "Three-line pass rule enabled".to_owned()
                }
                ("offsideline", "blue") => "Blue line set as offside line".to_owned(),
                ("offsideline", "center") => "Center line set as offside line".to_owned(),
                ("mercy", "0" | "off") => "Mercy rule disabled".to_owned(),
                ("mercy", _) => format!("Mercy rule set to {value} goals"),
                ("first", "0" | "off") => "First-to-goals rule disabled".to_owned(),
                ("first", _) => format!("First-to-goals rule set to {value} goals"),
                ("goalreplay", "on") => "Goal replays enabled".to_owned(),
                ("goalreplay", "off") => "Goal replays disabled".to_owned(),
                ("teamsize", _) => format!("Team size set to {value}"),
                _ => format!("{setting} changed to {value}"),
            };
            ctx.messages
                .add_server_chat_message(format!("{message} by {admin_name}"));
        }
    }
}

fn parse_clock(value: &str) -> Option<u32> {
    let (minutes, seconds): (u32, &str) = match value.split_once(':') {
        Some((minutes, seconds)) => (minutes.parse::<u32>().ok()?, seconds),
        None => (0, value),
    };
    let (seconds, centiseconds): (u32, u32) = match seconds.split_once('.') {
        Some((seconds, centiseconds)) => (
            seconds.parse::<u32>().ok()?,
            centiseconds.parse::<u32>().ok()? * if centiseconds.len() == 1 { 10 } else { 1 },
        ),
        None => (seconds.parse::<u32>().ok()?, 0),
    };
    Some(minutes * 6000 + seconds * 100 + centiseconds)
}

//! Runtime administration commands for the standard-match game mode.

use tracing::info;

use crate::game::PlayerId;
use crate::gamemode::GameContext;

use super::{
    BreakKind, BreakState, Faceoff, IcingConfiguration, Match, MatchSettings, MatchState,
    MatchTransition, OffsideConfiguration, OffsideLineConfiguration, StandardMatchGameMode,
    StandardMatchState, Stoppage, StoppageCause, TwoLinePassConfiguration, WarmupState,
};

/// Ticks from /faceoff until the faceoff.
const FACEOFF_COMMAND_DELAY: u32 = 500;

impl Match {
    /// Calls a center-ice faceoff in [FACEOFF_COMMAND_DELAY] ticks, and unpauses the game.
    pub(super) fn call_faceoff(
        &mut self,
        ctx: &mut GameContext,
        settings: &MatchSettings,
    ) -> Result<(), &'static str> {
        match &mut self.state {
            MatchState::Break(BreakState {
                kind: BreakKind::GameOver { .. },
                ..
            }) => return Err("The game is over"),
            // The intermission is shortened rather than replaced, as the clock is reset when it
            // ends.
            MatchState::Break(
                state @ BreakState {
                    kind: BreakKind::Intermission { .. },
                    ..
                },
            ) => {
                state.timer = FACEOFF_COMMAND_DELAY;
                state.paused = false;
            }
            _ => {
                // A goal replay that has not been queued yet would hold up the faceoff.
                self.game_state.replay = None;
                let stoppage = Self::admin_stoppage(FACEOFF_COMMAND_DELAY);
                self.apply(ctx, settings, MatchTransition::Break(stoppage));
            }
        }
        Ok(())
    }
    /// Holds the break timer, or stops play if it is on. Play resumes with a center-ice faceoff.
    pub(super) fn pause(
        &mut self,
        ctx: &mut GameContext,
        settings: &MatchSettings,
    ) -> Result<(), &'static str> {
        match &mut self.state {
            MatchState::Break(state) if state.paused => return Err("The game is already paused"),
            MatchState::Break(state) => state.paused = true,
            MatchState::Playing(_) => {
                let mut stoppage = Self::admin_stoppage(settings.config.time_break * 100);
                stoppage.paused = true;
                self.apply(ctx, settings, MatchTransition::Break(stoppage));
            }
        }
        Ok(())
    }
    /// Lets the break timer run again, with at least a full break left so that players are not
    /// caught by the faceoff.
    pub(super) fn unpause(
        &mut self,
        _ctx: &mut GameContext,
        settings: &MatchSettings,
    ) -> Result<(), &'static str> {
        match &mut self.state {
            MatchState::Break(state) if state.paused => {
                state.paused = false;
                state.timer = state.timer.max(settings.config.time_break * 100);
                Ok(())
            }
            _ => Err("The game is not paused"),
        }
    }
    fn admin_stoppage(timer: u32) -> BreakState {
        BreakState::new(
            timer,
            BreakKind::Stoppage(Stoppage {
                cause: StoppageCause::Admin,
                next_faceoff: Faceoff::Center,
            }),
        )
    }
}

impl StandardMatchGameMode {
    /// Ends warmup and starts the game, whether or not both teams have players or the warmup
    /// is paused.
    pub(super) fn start(&mut self, _ctx: &mut GameContext) -> Result<(), &'static str> {
        let StandardMatchState::WaitingForGame(warmup) = &self.state else {
            return Err("The game has already started");
        };
        self.start_game(warmup.score);
        Ok(())
    }
    pub(super) fn call_faceoff(&mut self, ctx: &mut GameContext) -> Result<(), &'static str> {
        let StandardMatchState::Game(game) = &mut self.state else {
            return Err("No game is currently ongoing");
        };
        game.call_faceoff(ctx, &self.settings)
    }
    pub(super) fn pause(&mut self, ctx: &mut GameContext) -> Result<(), &'static str> {
        match &mut self.state {
            StandardMatchState::WaitingForGame(warmup) if warmup.paused => {
                Err("The game is already paused")
            }
            StandardMatchState::WaitingForGame(warmup) => {
                warmup.paused = true;
                Ok(())
            }
            StandardMatchState::Game(game) => game.pause(ctx, &self.settings),
        }
    }
    pub(super) fn unpause(&mut self, ctx: &mut GameContext) -> Result<(), &'static str> {
        match &mut self.state {
            // The game opens with an intermission, so there is no need to hold back the start.
            StandardMatchState::WaitingForGame(warmup) if warmup.paused => {
                warmup.paused = false;
                Ok(())
            }
            StandardMatchState::WaitingForGame(_) => Err("The game is not paused"),
            StandardMatchState::Game(game) => game.unpause(ctx, &self.settings),
        }
    }
    /// Runs an admin command, and announces it with `done` if it succeeded.
    pub(super) fn handle_admin_command(
        &mut self,
        ctx: &mut GameContext,
        player: PlayerId,
        done: &str,
        command: fn(&mut Self, &mut GameContext) -> Result<(), &'static str>,
    ) {
        let Some(admin_name) = ctx.require_admin(player) else {
            return;
        };
        match command(self, ctx) {
            Ok(()) => {
                info!("{} by {} ({})", done, admin_name, player);
                ctx.messages
                    .add_server_chat_message(format!("{done} by {admin_name}"));
            }
            Err(message) => ctx
                .messages
                .add_directed_server_chat_message(message, player),
        }
    }
    /// Tells a player which rules are in use.
    pub(super) fn handle_rules(&self, ctx: &mut GameContext, player: PlayerId) {
        let config = &self.settings.config;
        let offside_line = if config.offside != OffsideConfiguration::Off
            && config.offside_line == OffsideLineConfiguration::Center
        {
            " (center line)"
        } else {
            ""
        };
        let mut messages = vec![format!(
            "{}{offside_line}, {}",
            config.offside.description(),
            config.icing.description(),
        )];
        if config.twoline_pass != TwoLinePassConfiguration::Off {
            messages.push(config.twoline_pass.description().to_owned());
        }
        if config.mercy > 0 {
            messages.push(format!(
                "Game ends when a team leads by {} goals",
                config.mercy
            ));
        }
        if config.first_to > 0 {
            messages.push(format!(
                "Game ends when a team reaches {} goals",
                config.first_to
            ));
        }
        for message in messages {
            ctx.messages
                .add_directed_server_chat_message(message, player);
        }
    }
    pub(super) fn handle_set(&mut self, ctx: &mut GameContext, arg: &str, player: PlayerId) {
        let Some(admin_name) = ctx.require_admin(player) else {
            return;
        };
        let old_config = self.settings.config;
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
                if let Ok(period) = value.parse()
                    && let StandardMatchState::Game(game) = &mut self.state
                {
                    if period == 0 {
                        // Period 0 is warmup. The score is kept, and the game starts again when
                        // warmup is over.
                        let score = game.game_state.score;
                        self.state = StandardMatchState::WaitingForGame(WarmupState {
                            score,
                            ..WarmupState::new()
                        });
                    } else {
                        game.game_state.period = period;
                    }
                }
            }
            "periodnum" => {
                if let Ok(periods) = value.parse() {
                    self.settings.config.periods = periods;
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
                "on" | "touch" => self.settings.config.icing = IcingConfiguration::Touch,
                "notouch" => self.settings.config.icing = IcingConfiguration::NoTouch,
                "off" => self.settings.config.icing = IcingConfiguration::Off,
                _ => {}
            },
            "offside" => match value {
                "on" | "delayed" => self.settings.config.offside = OffsideConfiguration::Delayed,
                "imm" | "immediate" => {
                    self.settings.config.offside = OffsideConfiguration::Immediate
                }
                "off" => self.settings.config.offside = OffsideConfiguration::Off,
                _ => {}
            },
            "twolinepass" => match value {
                "off" => self.settings.config.twoline_pass = TwoLinePassConfiguration::Off,
                "on" => self.settings.config.twoline_pass = TwoLinePassConfiguration::On,
                "forward" => self.settings.config.twoline_pass = TwoLinePassConfiguration::Forward,
                "double" | "both" => {
                    self.settings.config.twoline_pass = TwoLinePassConfiguration::Double
                }
                "blue" | "three" | "threeline" => {
                    self.settings.config.twoline_pass = TwoLinePassConfiguration::ThreeLine
                }
                _ => {}
            },
            "offsideline" => match value {
                "blue" => {
                    self.settings.config.offside_line = OffsideLineConfiguration::OffensiveBlue
                }
                "center" => self.settings.config.offside_line = OffsideLineConfiguration::Center,
                _ => {}
            },
            "mercy" => {
                if let Some(goals) = parse_goal_rule(value) {
                    self.settings.config.mercy = goals;
                }
            }
            "first" => {
                if let Some(goals) = parse_goal_rule(value) {
                    self.settings.config.first_to = goals;
                }
            }
            "goalreplay" => match value {
                "on" => self.settings.config.goal_replay = true,
                "off" => self.settings.config.goal_replay = false,
                _ => {}
            },
            "spawnoffset" => {
                if let Ok(offset) = value.parse() {
                    self.settings.config.spawn_point_offset = offset;
                }
            }
            "spawnplayeraltitude" => {
                if let Ok(altitude) = value.parse() {
                    self.settings.config.spawn_player_altitude = altitude;
                }
            }
            "spawnpuckaltitude" => {
                if let Ok(altitude) = value.parse() {
                    self.settings.config.spawn_puck_altitude = altitude;
                }
            }
            "spawnplayerkeepstick" => match value {
                "on" | "true" => self.settings.config.spawn_keep_stick_position = true,
                "off" | "false" => self.settings.config.spawn_keep_stick_position = false,
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
        if self.settings.config != old_config
            || self.teams.team_max != old_team_max
            || new_state_values != old_state_values
        {
            let message = match (setting, value) {
                ("redscore", _) => format!("Red score changed to {value}"),
                ("bluescore", _) => format!("Blue score changed to {value}"),
                ("period", _) => format!("Period set to {value}"),
                ("periodnum", _) => format!("Number of periods set to {value}"),
                ("clock", _) => format!("Clock set to {value}"),
                ("icing", _) => self.settings.config.icing.description().to_owned(),
                ("offside", _) => self.settings.config.offside.description().to_owned(),
                ("twolinepass", _) => self.settings.config.twoline_pass.description().to_owned(),
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

/// Parses the goal count of the mercy and first-to rules, where "off" is the same as 0.
fn parse_goal_rule(value: &str) -> Option<u32> {
    if value == "off" {
        Some(0)
    } else {
        value.parse().ok()
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

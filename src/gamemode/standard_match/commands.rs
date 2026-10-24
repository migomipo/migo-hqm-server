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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::Team;
    use crate::gamemode::standard_match::MatchConfiguration;
    use crate::gamemode::standard_match::test_game::{
        MatchGame, PUCK_LANE, ice_the_puck, match_server,
    };
    use glam::Vec3;

    #[test]
    fn rules_shows_the_offside_and_icing_settings() {
        let mut t = match_server();
        let player = t.join("player");
        t.take_messages(player);
        t.chat(player, "/rules");
        assert_eq!(t.take_chats(player), ["Offside disabled, Icing disabled"]);

        let config = &mut t.mode.settings.config;
        config.offside = OffsideConfiguration::Immediate;
        config.offside_line = OffsideLineConfiguration::Center;
        config.icing = IcingConfiguration::NoTouch;
        config.twoline_pass = TwoLinePassConfiguration::ThreeLine;
        config.mercy = 5;
        config.first_to = 7;
        t.chat(player, "/rules");
        assert_eq!(
            t.take_chats(player),
            [
                "Immediate offside enabled (center line), No-touch icing enabled",
                "Three-line pass rule enabled",
                "Game ends when a team leads by 5 goals",
                "Game ends when a team reaches 7 goals",
            ]
        );
    }

    #[test]
    fn fs_forces_a_player_off_the_ice() {
        let mut game = MatchGame::start(MatchGame::quick_config(), 1, 1);
        let blue = game.blue[0];
        game.chat(game.admin(), &format!("/fs {}", blue.index));
        assert!(game.t.ctx().objects.skater(blue).is_none());
    }

    #[test]
    fn start_starts_the_game_during_warmup() {
        let mut game = MatchGame::join(MatchGame::quick_config(), 1, 1);
        game.ticks(5);
        assert_eq!(game.scoreboard.period, 0);
        game.chat(game.admin(), "/start");
        game.tick_until(500, |game| game.scoreboard.period >= 1);
        game.chat(game.admin(), "/start");
        assert_eq!(
            game.heard,
            ["Game started by red0", "The game has already started"]
        );
    }

    #[test]
    fn start_starts_the_game_without_players_on_both_teams() {
        let mut game = MatchGame::join(MatchGame::quick_config(), 1, 0);
        game.ticks(5);
        game.chat(game.admin(), "/start");
        game.play_until_faceoff();
        assert_eq!(game.scoreboard.period, 1);
    }

    #[test]
    fn pause_stops_play_until_unpause_and_play_resumes_with_a_faceoff() {
        let mut game = MatchGame::start(MatchGame::quick_config(), 1, 1);
        game.set_puck(Vec3::new(PUCK_LANE, 0.5, 10.0), Vec3::ZERO);
        game.chat(game.admin(), "/pause");
        game.tick();
        assert!(!game.is_playing());
        let time = game.scoreboard.time;
        // Much longer than the break, which stands still while paused.
        game.ticks(500);
        assert!(!game.is_playing());
        assert_eq!(game.scoreboard.time, time);

        // Play resumes after a full break, so that players are not caught by the faceoff.
        game.chat(game.admin(), "/unpause");
        let ticks = game.tick_until(1000, |game| game.is_playing());
        assert!(ticks >= 99, "{ticks}");
        assert!(is_center_faceoff(&game), "{:?}", game.pucks());
        game.ticks(50);
        assert!(game.scoreboard.time < time);
        assert_eq!(game.heard, ["Game paused by red0", "Game resumed by red0"]);
    }

    #[test]
    fn pause_holds_a_stoppage_until_unpause() {
        let mut game = MatchGame::start(MatchGame::quick_config(), 1, 1);
        game.score(game.red[0]);
        game.chat(game.admin(), "/pause");
        game.tick();
        let goal_message_timer = game.scoreboard.goal_message_timer;
        game.ticks(500);
        assert!(!game.is_playing());
        assert_eq!(game.scoreboard.goal_message_timer, goal_message_timer);
        game.chat(game.admin(), "/unpause");
        game.tick_until(1000, |game| game.is_playing());
    }

    #[test]
    fn pause_and_unpause_tell_the_admin_when_they_do_nothing() {
        let mut game = MatchGame::join(MatchGame::quick_config(), 1, 1);
        let blue = game.blue[0];
        game.t.take_messages(blue);
        game.play_until_faceoff();
        game.chat(game.admin(), "/unpause");
        game.chat(game.admin(), "/pause");
        game.chat(game.admin(), "/pause");
        game.chat(blue, "/unpause");
        assert_eq!(
            game.heard,
            [
                "The game is not paused",
                "Game paused by red0",
                "The game is already paused",
            ]
        );
        assert!(
            game.t
                .take_chats(blue)
                .contains(&"Please log in before using that command".to_owned())
        );
        let mut warmup = MatchGame::join(MatchGame::quick_config(), 1, 0);
        warmup.tick();
        warmup.chat(warmup.admin(), "/unpause");
        warmup.chat(warmup.admin(), "/pause");
        warmup.chat(warmup.admin(), "/pause");
        warmup.chat(warmup.admin(), "/faceoff");
        assert_eq!(
            warmup.heard,
            [
                "The game is not paused",
                "Game paused by red0",
                "The game is already paused",
                "No game is currently ongoing",
            ]
        );
    }

    #[test]
    fn pause_in_warmup_holds_back_the_game_until_unpause() {
        let mut game = MatchGame::join(MatchGame::quick_config(), 1, 1);
        game.tick();
        game.chat(game.admin(), "/pause");
        // Much longer than warmup, which would start the game with players on both teams.
        game.ticks(3000);
        assert_eq!(game.scoreboard.period, 0);
        game.chat(game.admin(), "/unpause");
        game.play_until_faceoff();
        assert_eq!(game.scoreboard.period, 1);
    }

    #[test]
    fn start_starts_the_game_from_a_paused_warmup() {
        let mut game = MatchGame::join(MatchGame::quick_config(), 1, 1);
        game.tick();
        game.chat(game.admin(), "/pause");
        game.chat(game.admin(), "/start");
        game.play_until_faceoff();
        assert_eq!(game.scoreboard.period, 1);
    }

    /// Whether the puck is at the center faceoff spot.
    fn is_center_faceoff(game: &MatchGame) -> bool {
        let puck = game.pucks()[0];
        (puck.x - 15.0).abs() < 0.5 && (puck.z - 30.5).abs() < 0.5
    }

    #[test]
    fn faceoff_calls_a_center_ice_faceoff() {
        let mut game = MatchGame::start(MatchGame::quick_config(), 1, 1);
        game.set_puck(Vec3::new(PUCK_LANE, 0.5, 10.0), Vec3::ZERO);
        game.chat(game.admin(), "/faceoff");
        game.tick();
        assert!(!game.is_playing());
        let ticks = game.tick_until(1000, |game| game.is_playing());
        assert!((495..=500).contains(&ticks), "{ticks}");
        assert!(is_center_faceoff(&game), "{:?}", game.pucks());
        assert!(game.heard("Faceoff initiated by red0"), "{:?}", game.heard);
    }

    #[test]
    fn faceoff_unpauses_the_game() {
        let mut game = MatchGame::start(MatchGame::quick_config(), 1, 1);
        game.chat(game.admin(), "/pause");
        game.chat(game.admin(), "/faceoff");
        game.tick_until(1000, |game| game.is_playing());
    }

    #[test]
    fn faceoff_during_an_intermission_shortens_it_and_resets_the_clock() {
        let config = MatchConfiguration {
            time_intermission: 60,
            ..MatchGame::quick_config()
        };
        let mut game = MatchGame::join(config, 1, 1);
        // The game opens with an intermission.
        game.tick_until(10_000, |game| game.scoreboard.period == 1);
        game.chat(game.admin(), "/faceoff");
        game.tick_until(510, |game| game.is_playing());
        assert!(
            game.scoreboard.time > config.time_period * 100 - 5,
            "{}",
            game.scoreboard.time
        );
    }

    #[test]
    fn faceoff_after_a_goal_skips_the_goal_replay() {
        let config = MatchConfiguration {
            goal_replay: true,
            ..MatchGame::quick_config()
        };
        let mut game = MatchGame::start(config, 1, 1);
        game.score(game.red[0]);
        game.chat(game.admin(), "/faceoff");
        let ticks = game.tick_until(1000, |game| game.is_playing());
        assert!(ticks <= 500, "{ticks}");
        assert!(!game.heard("Goal replay"), "{:?}", game.heard);
    }

    #[test]
    fn faceoff_is_refused_after_the_game_is_over() {
        let config = MatchConfiguration {
            first_to: 1,
            ..MatchGame::quick_config()
        };
        let mut game = MatchGame::start(config, 1, 1);
        game.score(game.red[0]);
        assert!(game.scoreboard.game_over);
        game.heard.clear();
        game.chat(game.admin(), "/faceoff");
        assert_eq!(game.heard, ["The game is over"]);
    }

    #[test]
    fn set_clock_period_and_scores() {
        let mut game = MatchGame::start(MatchGame::quick_config(), 1, 1);
        let admin = game.admin();
        game.chat(admin, "/set clock 1:30");
        game.chat(admin, "/set period 2");
        game.chat(admin, "/set redscore 3");
        game.chat(admin, "/set bluescore 4");
        game.tick();
        let scoreboard = &game.scoreboard;
        assert!(
            (8990..=9000).contains(&scoreboard.time),
            "{}",
            scoreboard.time
        );
        assert_eq!(scoreboard.period, 2);
        assert_eq!((scoreboard.red_score, scoreboard.blue_score), (3, 4));
    }

    #[test]
    fn set_period_zero_returns_to_warmup() {
        let config = MatchConfiguration {
            icing: IcingConfiguration::NoTouch,
            ..MatchGame::quick_config()
        };
        let mut game = MatchGame::start(config, 1, 1);
        game.chat(game.admin(), "/set redscore 2");
        game.chat(game.admin(), "/set period 0");
        game.tick();
        assert_eq!(game.scoreboard.period, 0);
        assert_eq!(game.scoreboard.red_score, 2);
        // Warmup has no rules, so icing the puck is not called.
        ice_the_puck(&mut game);
        assert!(!game.heard("Icing"), "{:?}", game.heard);

        // The game starts again after warmup, with the score kept.
        game.play_until_faceoff();
        assert_eq!(game.scoreboard.period, 1);
        assert_eq!(game.scoreboard.red_score, 2);
    }

    #[test]
    fn set_periodnum_sets_the_number_of_periods() {
        let config = MatchConfiguration {
            time_period: 5,
            ..MatchGame::quick_config()
        };
        let mut game = MatchGame::start(config, 1, 1);
        game.chat(game.admin(), "/set periodnum 1");
        game.score(game.red[0]);
        game.tick_until(1000, |game| game.scoreboard.period == 2);
        assert!(game.scoreboard.game_over);
    }

    #[test]
    fn set_rule_settings_announce_the_new_rules() {
        let mut game = MatchGame::join(MatchGame::quick_config(), 1, 0);
        let admin = game.admin();
        let mut failures = vec![];
        for (command, message) in [
            ("icing on", "Touch icing enabled"),
            ("icing notouch", "No-touch icing enabled"),
            ("icing off", "Icing disabled"),
            ("offside on", "Offside enabled"),
            ("offside imm", "Immediate offside enabled"),
            ("offside off", "Offside disabled"),
            ("offside immediate", "Immediate offside enabled"),
            ("offside off", "Offside disabled"),
            ("offsideline center", "Center line set as offside line"),
            ("offsideline blue", "Blue line set as offside line"),
            ("twolinepass on", "Regular two-line pass rule enabled"),
            ("twolinepass forward", "Forward two-line pass rule enabled"),
            (
                "twolinepass both",
                "Regular and forward two-line pass rules enabled",
            ),
            ("twolinepass three", "Three-line pass rule enabled"),
            ("twolinepass off", "Two-line pass rule disabled"),
            ("mercy 3", "Mercy rule set to 3 goals"),
            ("mercy 0", "Mercy rule disabled"),
            ("first 5", "First-to-goals rule set to 5 goals"),
            ("first 0", "First-to-goals rule disabled"),
            ("goalreplay on", "Goal replays enabled"),
            ("goalreplay off", "Goal replays disabled"),
            ("teamsize 3", "Team size set to 3"),
        ] {
            game.heard.clear();
            game.chat(admin, &format!("/set {command}"));
            let expected = format!("{message} by red0");
            if !game.heard(&expected) {
                failures.push(format!("/set {command}: {:?}", game.heard));
            }
        }
        assert!(failures.is_empty(), "{failures:#?}");
    }

    #[test]
    fn set_mercy_and_first_off_disable_the_rules() {
        let mut game = MatchGame::join(MatchGame::quick_config(), 1, 0);
        let admin = game.admin();
        for (enable, disable, message) in [
            ("mercy 3", "mercy off", "Mercy rule disabled by red0"),
            (
                "first 5",
                "first off",
                "First-to-goals rule disabled by red0",
            ),
        ] {
            game.chat(admin, &format!("/set {enable}"));
            game.heard.clear();
            game.chat(admin, &format!("/set {disable}"));
            assert!(game.heard(message), "/set {disable}: {:?}", game.heard);
        }
    }

    #[test]
    fn set_icing_changes_the_icing_rule() {
        let mut game = MatchGame::start(MatchGame::quick_config(), 1, 1);
        game.chat(game.admin(), "/set icing notouch");
        ice_the_puck(&mut game);
        assert!(game.heard("Icing"), "{:?}", game.heard);
    }

    #[test]
    fn set_teamsize_limits_the_players_on_each_team() {
        let mut game = MatchGame::join(MatchGame::quick_config(), 1, 0);
        game.chat(game.admin(), "/set teamsize 1");
        let second = game.t.join("second");
        game.t.press_join(second, Team::Red);
        game.ticks(5);
        assert_eq!(game.t.ctx().players.count_team_members(), (1, 0));
    }

    #[test]
    fn set_spawn_settings_apply_at_the_next_faceoff() {
        let mut game = MatchGame::join(MatchGame::quick_config(), 1, 1);
        let admin = game.admin();
        game.chat(admin, "/set spawnoffset 4");
        game.chat(admin, "/set spawnplayeraltitude 4");
        game.chat(admin, "/set spawnpuckaltitude 3");
        game.play_until_faceoff();
        let center_z = game.t.server.rink.center_line.z;
        let red = game.skater_pos(game.red[0]);
        assert!(((red.z - center_z) - 4.0).abs() < 0.1, "{red}");
        assert!((red.y - 4.0).abs() < 0.1, "{red}");
        assert!((game.pucks()[0].y - 3.0).abs() < 0.1);
    }
}

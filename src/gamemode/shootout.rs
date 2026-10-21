use glam::Vec3;
use glamx::Rot3;
use std::f32::consts::PI;
use tracing::info;

use crate::game::{PhysicsEvent, PlayerId, RulesState};
use crate::game::{PlayerIndex, Puck, PuckId, ScoreboardValues, Team};
use crate::gamemode::util::{GameProgress, SpawnPoint, TeamSelection, get_spawnpoint};
use crate::gamemode::{CommandResult, ExitReason, GameContext, GameMode};

/// Ticks of warmup once both teams have players, before the countdown to the first attempt.
const WARMUP_TIME: u32 = 1000;
/// Ticks between the end of warmup and the first attempt.
const START_DELAY: u32 = 500;
/// Ticks the attacker has for an attempt.
const ATTEMPT_TIME: u32 = 2000;
/// Ticks between the end of an attempt and the next one.
const PAUSE_BETWEEN_ATTEMPTS: u32 = 500;

#[derive(Debug, Clone)]
enum ServerStatus {
    WaitingForGame { time: u32, time_until_start: u32 },
    Game(ShootoutGame),
}

impl ServerStatus {
    /// Waiting for players, with the countdown to the first attempt not yet started.
    fn waiting() -> Self {
        ServerStatus::WaitingForGame {
            time: WARMUP_TIME,
            time_until_start: START_DELAY,
        }
    }
}

#[derive(Debug, Clone)]
struct ShootoutGame {
    state: ShootoutAttemptState,
    game_state: ShootoutGameState,
}

#[derive(Debug, Clone)]
struct ShootoutGameState {
    round: u32,
    team: Team,
    red_score: u32,
    blue_score: u32,
}

impl ShootoutGameState {
    fn finish_attempt(&mut self, config: &ShootoutGameConfiguration, goal_scored: bool) -> bool {
        if goal_scored {
            match self.team {
                Team::Red => self.red_score += 1,
                Team::Blue => self.blue_score += 1,
            }
        }

        let red_attempts_taken = self.round + 1;
        let blue_attempts_taken = self.round
            + match self.team {
                Team::Red => 0,
                Team::Blue => 1,
            };
        let attempts = config.attempts.max(red_attempts_taken);
        let remaining_red_attempts = attempts - red_attempts_taken;
        let remaining_blue_attempts = attempts - blue_attempts_taken;

        if let Some(difference) = self.red_score.checked_sub(self.blue_score) {
            remaining_blue_attempts < difference
        } else if let Some(difference) = self.blue_score.checked_sub(self.red_score) {
            remaining_red_attempts < difference
        } else {
            false
        }
    }

    fn start_next_attempt(&mut self) {
        self.team = self.team.get_other_team();
        if self.team == Team::Red {
            self.round += 1;
        }
    }
}

#[derive(Debug, Clone)]
pub struct ShootoutGameConfiguration {
    pub attempts: u32,
}

pub struct ShootoutGameMode {
    config: ShootoutGameConfiguration,
    status: ServerStatus,
    teams: TeamSelection,
}

#[derive(Debug, Clone)]
struct AttackState {
    time: u32,
    progress: f32,
    no_more_touch: bool,
    /// The puck of this attempt, if it could be spawned.
    puck: Option<PuckId>,
}

#[derive(Debug, Clone)]
struct OverState {
    timer_until_next_round: u32,
    time_at_end: u32,
    goal_scored: bool,
    game_over: bool,
}

#[derive(Debug, Clone)]
enum ShootoutAttemptState {
    Attack(AttackState),
    Over(OverState), // Attempt is over
}

impl ShootoutAttemptState {
    fn scoreboard_values(&self) -> (u32, u32, bool) {
        match self {
            Self::Attack(AttackState { time, .. }) => (*time, 0, false),
            Self::Over(OverState {
                timer_until_next_round,
                goal_scored,
                time_at_end,
                game_over,
            }) => (
                (*time_at_end).max(1),
                if *goal_scored {
                    *timer_until_next_round
                } else {
                    0
                },
                *game_over,
            ),
        }
    }
}

/// A change to the state of a running shootout.
///
/// The current attempt state decides which transition to make, and [ShootoutGame::apply]
/// carries it out.
#[derive(Debug, Clone, Copy)]
enum ShootoutTransition {
    Continue,
    /// The attempt has ended, with the time that was left on the clock.
    EndAttempt {
        time: u32,
        goal_scored: bool,
    },
    StartNextAttempt,
    /// The shootout is over and a new game should start.
    Finish,
}

impl AttackState {
    fn setup_attempt(
        ctx: &mut GameContext,
        config: &ShootoutGameConfiguration,
        game_state: &ShootoutGameState,
    ) -> AttackState {
        let team = game_state.team;
        let defending_team = team.get_other_team();

        let remaining_attempts = config.attempts.saturating_sub(game_state.round);
        let msg = if remaining_attempts >= 2 {
            format!("{remaining_attempts} attempts left for {team}")
        } else if remaining_attempts == 1 {
            format!("Last attempt for {team}")
        } else {
            format!("Tie-breaker round for {team}")
        };
        ctx.messages.add_server_chat_message(msg);
        ctx.objects.remove_all_pucks();

        let length = ctx.rink.length;
        let width = ctx.rink.width;

        let puck_pos = Vec3::new(width / 2.0, 1.0, length / 2.0);
        let puck = ctx.objects.spawn_puck(Puck::new(puck_pos, Rot3::IDENTITY));

        let mut red_players = vec![];
        let mut blue_players = vec![];

        for player in ctx.players.iter() {
            let player_id = player.id;
            if let Some(team) = player.team() {
                if team == Team::Red {
                    red_players.push(player_id);
                } else if team == Team::Blue {
                    blue_players.push(player_id);
                }
            }
        }

        let red_rot = Rot3::IDENTITY;

        let blue_rot = Rot3::from_rotation_y(PI);

        let red_goalie_pos = Vec3::new(width / 2.0, 1.5, length - 5.0);
        let blue_goalie_pos = Vec3::new(width / 2.0, 1.5, 5.0);
        let (attacking_players, defending_players, attacking_rot, defending_rot, goalie_pos) =
            match team {
                Team::Red => (
                    red_players,
                    blue_players,
                    red_rot,
                    blue_rot,
                    blue_goalie_pos,
                ),
                Team::Blue => (blue_players, red_players, blue_rot, red_rot, red_goalie_pos),
            };
        let center_pos = Vec3::new(width / 2.0, 1.5, length / 2.0);
        for (index, player_id) in attacking_players.into_iter().enumerate() {
            let mut pos = center_pos + attacking_rot * Vec3::new(0.0, 0.0, 3.0);
            if index > 0 {
                let dist = ((index / 2) + 1) as f32;

                let side = Vec3::new(-1.5 * dist, 0.0, 0.0);
                pos += attacking_rot * side;
            }
            ctx.spawn_skater(player_id, team, pos, attacking_rot, false);
        }
        for (index, player_id) in defending_players.into_iter().enumerate() {
            let mut pos = goalie_pos;
            if index > 0 {
                let dist = ((index / 2) + 1) as f32;

                let side = Vec3::new(-1.5 * dist, 0.0, 0.0);
                pos += defending_rot * side;
            }
            ctx.spawn_skater(player_id, defending_team, pos, defending_rot, false);
        }
        Self {
            time: ATTEMPT_TIME,
            progress: 0.0,
            no_more_touch: false,
            puck,
        }
    }

    fn before_tick(&mut self) -> ShootoutTransition {
        self.time = self.time.saturating_sub(1);
        if self.time == 0 {
            ShootoutTransition::EndAttempt {
                time: 0,
                goal_scored: false,
            }
        } else {
            ShootoutTransition::Continue
        }
    }

    fn tick(
        &mut self,
        ctx: &mut GameContext,
        attacking_team: Team,
        events: &[PhysicsEvent],
    ) -> ShootoutTransition {
        for event in events {
            match event {
                PhysicsEvent::PuckEnteredNet { team: net_team, .. } => {
                    return ShootoutTransition::EndAttempt {
                        time: self.time,
                        goal_scored: net_team.get_other_team() == attacking_team,
                    };
                }
                PhysicsEvent::PuckPassedGoalLine { .. } => {
                    return ShootoutTransition::EndAttempt {
                        time: self.time,
                        goal_scored: false,
                    };
                }
                PhysicsEvent::PuckTouch { player, .. } => {
                    if let Some(touching_team) =
                        ctx.players.get(*player).and_then(|player| player.team())
                    {
                        if touching_team == attacking_team && self.no_more_touch {
                            return ShootoutTransition::EndAttempt {
                                time: self.time,
                                goal_scored: false,
                            };
                        } else if touching_team != attacking_team {
                            self.no_more_touch = true;
                        }
                    }
                }
                PhysicsEvent::PuckTouchedNet { team: net_team, .. } => {
                    if net_team.get_other_team() == attacking_team {
                        self.no_more_touch = true;
                    }
                }
                _ => {}
            }
        }

        let center_pos = Vec3::new(ctx.rink.width / 2.0, 0.0, ctx.rink.length / 2.0);
        let normal = match attacking_team {
            Team::Red => Vec3::NEG_Z,
            Team::Blue => Vec3::Z,
        };
        let Some(puck) = self.puck.and_then(|puck| ctx.objects.get_puck(puck)) else {
            return ShootoutTransition::Continue;
        };
        let progress = (puck.body.pos - center_pos).dot(normal);
        if !self.no_more_touch && progress > self.progress {
            self.progress = progress;
            ShootoutTransition::Continue
        } else if progress - self.progress < if self.no_more_touch { -5.0 } else { -0.5 } {
            ShootoutTransition::EndAttempt {
                time: self.time,
                goal_scored: false,
            }
        } else {
            ShootoutTransition::Continue
        }
    }
}

impl OverState {
    fn before_tick(&mut self) -> ShootoutTransition {
        self.timer_until_next_round = self.timer_until_next_round.saturating_sub(1);
        if self.timer_until_next_round != 0 {
            ShootoutTransition::Continue
        } else if self.game_over {
            ShootoutTransition::Finish
        } else {
            ShootoutTransition::StartNextAttempt
        }
    }
}

impl ShootoutGame {
    fn start(ctx: &mut GameContext, config: &ShootoutGameConfiguration) -> Self {
        let game_state = ShootoutGameState {
            round: 0,
            team: Team::Red,
            red_score: 0,
            blue_score: 0,
        };
        let attack = AttackState::setup_attempt(ctx, config, &game_state);
        ShootoutGame {
            state: ShootoutAttemptState::Attack(attack),
            game_state,
        }
    }

    fn scoreboard(&self) -> ScoreboardValues {
        let (time, goal_message_timer, game_over) = self.state.scoreboard_values();
        ScoreboardValues {
            rules_state: RulesState::Regular {
                offside_warning: false,
                icing_warning: false,
            },
            red_score: self.game_state.red_score,
            blue_score: self.game_state.blue_score,
            period: 1,
            time,
            goal_message_timer,
            game_over,
        }
    }
}

impl ShootoutGame {
    fn before_tick(
        &mut self,
        ctx: &mut GameContext,
        config: &ShootoutGameConfiguration,
    ) -> GameProgress {
        let transition = match &mut self.state {
            ShootoutAttemptState::Attack(state) => state.before_tick(),
            ShootoutAttemptState::Over(state) => state.before_tick(),
        };
        self.apply(ctx, config, transition)
    }

    fn tick(
        &mut self,
        ctx: &mut GameContext,
        config: &ShootoutGameConfiguration,
        events: &[PhysicsEvent],
    ) -> GameProgress {
        let transition = match &mut self.state {
            ShootoutAttemptState::Attack(state) => state.tick(ctx, self.game_state.team, events),
            ShootoutAttemptState::Over(_) => ShootoutTransition::Continue,
        };
        self.apply(ctx, config, transition)
    }

    /// Carries out a transition. This is the only place where a running shootout changes state.
    fn apply(
        &mut self,
        ctx: &mut GameContext,
        config: &ShootoutGameConfiguration,
        transition: ShootoutTransition,
    ) -> GameProgress {
        match transition {
            ShootoutTransition::Continue => {}
            ShootoutTransition::EndAttempt { time, goal_scored } => {
                let game_over = self.game_state.finish_attempt(config, goal_scored);
                if goal_scored {
                    ctx.add_goal_message(self.game_state.team, None, None);
                } else {
                    ctx.messages.add_server_chat_message("Miss");
                }
                self.state = ShootoutAttemptState::Over(OverState {
                    timer_until_next_round: PAUSE_BETWEEN_ATTEMPTS,
                    time_at_end: time,
                    goal_scored,
                    game_over,
                });
            }
            ShootoutTransition::StartNextAttempt => {
                self.game_state.start_next_attempt();
                self.state = ShootoutAttemptState::Attack(AttackState::setup_attempt(
                    ctx,
                    config,
                    &self.game_state,
                ));
            }
            ShootoutTransition::Finish => return GameProgress::Finished,
        }
        GameProgress::Continue
    }
}

impl ShootoutGameMode {
    pub fn new(config: ShootoutGameConfiguration) -> Self {
        ShootoutGameMode {
            config,
            status: ServerStatus::waiting(),
            teams: TeamSelection::new(1),
        }
    }

    /// Ends the current game and resets the game mode with it, so that the rest of this tick
    /// does not run the finished game's logic on the cleared server state.
    fn finish_game(&mut self, ctx: &mut GameContext) {
        ctx.new_game();
        self.status = ServerStatus::waiting();
    }

    fn start_game(&mut self, ctx: &mut GameContext) {
        let game = ShootoutGame::start(ctx, &self.config);
        self.status = ServerStatus::Game(game);
    }

    fn update_players(&mut self, ctx: &mut GameContext) -> (usize, usize) {
        self.teams.update(ctx, |rink, team, _| {
            get_spawnpoint(rink, team, SpawnPoint::Bench)
        })
    }

    fn reset_game(&mut self, ctx: &mut GameContext, player_id: PlayerId) {
        if let Some(name) = ctx.require_admin(player_id) {
            info!("{} ({}) reset game", name, player_id);
            self.finish_game(ctx);
            ctx.messages
                .add_server_chat_message(format!("Game reset by {name}"));
        }
    }

    fn force_player_off_ice(
        &mut self,
        ctx: &mut GameContext,
        admin_player_id: PlayerId,
        force_player_index: PlayerIndex,
    ) {
        let Some(admin_player_name) = ctx.require_admin(admin_player_id) else {
            return;
        };
        let Some(force_player) = ctx.players.get_by_index(force_player_index) else {
            return;
        };
        let (force_player_id, force_player_name) = (force_player.id, force_player.name());
        if self.teams.force_spectate(ctx, force_player_id) {
            info!(
                "{} ({}) forced {} ({}) off ice",
                admin_player_name, admin_player_id, force_player_name, force_player_index
            );
            ctx.messages.add_server_chat_message(format!(
                "{force_player_name} forced off ice by {admin_player_name}"
            ));
        }
    }
}

impl GameMode for ShootoutGameMode {
    fn before_tick(&mut self, ctx: &mut GameContext) {
        let (red_player_count, blue_player_count) = self.update_players(ctx);
        match &mut self.status {
            ServerStatus::WaitingForGame {
                time,
                time_until_start,
            } => {
                if red_player_count > 0 && blue_player_count > 0 {
                    *time = time.saturating_sub(1);
                    if *time == 0 {
                        *time_until_start = time_until_start.saturating_sub(1);
                        if *time_until_start == 0 {
                            self.start_game(ctx);
                        }
                    }
                } else {
                    self.status = ServerStatus::waiting();
                }
            }
            ServerStatus::Game(game) => {
                if game.before_tick(ctx, &self.config) == GameProgress::Finished {
                    self.finish_game(ctx);
                }
            }
        }
    }

    fn after_tick(&mut self, ctx: &mut GameContext, events: &[PhysicsEvent]) -> ScoreboardValues {
        if let ServerStatus::Game(game) = &mut self.status
            && game.tick(ctx, &self.config, events) == GameProgress::Finished
        {
            self.finish_game(ctx);
        }
        match &self.status {
            &ServerStatus::WaitingForGame { time, .. } => ScoreboardValues {
                rules_state: RulesState::default(),
                red_score: 0,
                blue_score: 0,
                period: if time == 0 { 1 } else { 0 },
                time,
                goal_message_timer: 0,
                game_over: false,
            },
            ServerStatus::Game(game) => game.scoreboard(),
        }
    }

    fn handle_command(
        &mut self,
        ctx: &mut GameContext,
        cmd: &str,
        arg: &str,
        player_id: PlayerId,
    ) -> CommandResult {
        match cmd {
            "reset" | "resetgame" => {
                self.reset_game(ctx, player_id);
            }
            "fs" => {
                if let Ok(force_player_index) = arg.parse::<PlayerIndex>() {
                    self.force_player_off_ice(ctx, player_id, force_player_index);
                }
            }
            _ => return CommandResult::Unknown,
        }
        CommandResult::Handled
    }

    fn game_started(&mut self, ctx: &mut GameContext) {
        self.status = ServerStatus::waiting();
        let rink = &*ctx.rink;
        let width = rink.width;
        let length = rink.length;

        let pos = Vec3::new(width / 2.0, 1.5, length / 2.0);
        let rot = Rot3::IDENTITY;
        ctx.objects.spawn_puck(Puck::new(pos, rot));
    }

    fn game_abandoned(&mut self, _ctx: &mut GameContext) {
        self.status = ServerStatus::waiting();
    }

    fn before_player_exit(
        &mut self,
        _ctx: &mut GameContext,
        player_id: PlayerId,
        _reason: ExitReason,
    ) {
        self.teams.remove_player(player_id);
    }

    fn server_list_team_size(&self) -> u32 {
        self.teams.team_max as u32
    }

    fn include_tick_in_recording(&self) -> bool {
        !matches!(self.status, ServerStatus::WaitingForGame { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::test_server::{Received, TestServer};

    const CONFIG: ShootoutGameConfiguration = ShootoutGameConfiguration { attempts: 3 };

    fn finished_game() -> ShootoutGame {
        ShootoutGame {
            state: ShootoutAttemptState::Over(OverState {
                timer_until_next_round: 1,
                time_at_end: 0,
                goal_scored: false,
                game_over: true,
            }),
            game_state: ShootoutGameState {
                round: 2,
                team: Team::Blue,
                red_score: 2,
                blue_score: 0,
            },
        }
    }

    fn is_waiting(status: &ServerStatus) -> bool {
        matches!(
            status,
            ServerStatus::WaitingForGame {
                time: WARMUP_TIME,
                time_until_start: START_DELAY,
            }
        )
    }

    #[test]
    fn attempts_end_and_the_next_one_starts_for_the_other_team() {
        let mut server = crate::server::test_server();
        let mut ctx = server.context();
        let mut game = finished_game();
        game.game_state.round = 0;
        game.game_state.team = Team::Red;
        game.game_state.red_score = 0;
        game.state = ShootoutAttemptState::Attack(AttackState {
            time: 1,
            progress: 0.0,
            no_more_touch: false,
            puck: None,
        });

        assert_eq!(game.before_tick(&mut ctx, &CONFIG), GameProgress::Continue);
        let ShootoutAttemptState::Over(over) = &mut game.state else {
            panic!("the attempt should have ended");
        };
        assert!(!over.goal_scored);
        assert!(!over.game_over);
        over.timer_until_next_round = 1;

        assert_eq!(game.before_tick(&mut ctx, &CONFIG), GameProgress::Continue);
        assert!(matches!(game.state, ShootoutAttemptState::Attack(_)));
        assert_eq!(game.game_state.team, Team::Blue);
        assert_eq!(ctx.objects.pucks().count(), 1);
    }

    #[test]
    fn last_attempt_finishes_the_game() {
        let mut server = crate::server::test_server();
        let mut ctx = server.context();
        let mut game = finished_game();

        assert_eq!(game.before_tick(&mut ctx, &CONFIG), GameProgress::Finished);
    }

    #[test]
    fn finished_game_returns_to_waiting_in_the_same_tick() {
        let mut server = crate::server::test_server();
        let mut ctx = server.context();
        let mut mode = ShootoutGameMode::new(ShootoutGameConfiguration { attempts: 3 });
        mode.status = ServerStatus::Game(finished_game());
        ctx.objects
            .spawn_puck(Puck::new(Vec3::ZERO, Rot3::IDENTITY));

        mode.before_tick(&mut ctx);

        assert_eq!(ctx.objects.pucks().count(), 0);
        assert!(is_waiting(&mode.status));
    }

    #[test]
    fn reset_command_returns_to_waiting() {
        let mut t = TestServer::new(ShootoutGameMode::new(CONFIG));
        let admin = t.join_admin("admin");
        t.mode.status = ServerStatus::Game(finished_game());
        t.take_messages(admin);

        t.chat(admin, "/reset");

        assert!(is_waiting(&t.mode.status));
        assert_eq!(t.take_chats(admin), ["Game reset by admin"]);
    }

    fn attempt_over(t: &TestServer<ShootoutGameMode>) -> bool {
        matches!(
            &t.mode.status,
            ServerStatus::Game(ShootoutGame {
                state: ShootoutAttemptState::Over(_),
                ..
            })
        )
    }

    fn attacking(t: &TestServer<ShootoutGameMode>, team: Team) -> bool {
        matches!(
            &t.mode.status,
            ServerStatus::Game(game)
                if game.game_state.team == team
                    && matches!(game.state, ShootoutAttemptState::Attack(_))
        )
    }

    #[test]
    fn a_shootout_runs_from_joining_teams_to_the_next_game() {
        let config = ShootoutGameConfiguration { attempts: 1 };
        let mut t = TestServer::new(ShootoutGameMode::new(config));
        let red = t.join("red");
        let blue = t.join("blue");
        t.press_join(red, Team::Red);
        t.press_join(blue, Team::Blue);
        t.tick();
        assert_eq!(t.ctx().players.count_team_members(), (1, 1));
        t.take_messages(red);

        // Warmup runs out and the first attempt starts.
        t.tick_until(WARMUP_TIME + START_DELAY, |t| attacking(t, Team::Red));
        assert_eq!(t.take_chats(red), ["Last attempt for Red"]);

        // Move the defender aside and slide the puck into the blue net.
        let mut ctx = t.ctx();
        let center_x = ctx.rink.width / 2.0;
        let defender = ctx.objects.skater_mut(blue).unwrap();
        defender.teleport(Vec3::new(1.0, 1.5, 5.0), Rot3::IDENTITY);
        let (_, puck) = ctx.objects.pucks_mut().next().unwrap();
        puck.body.pos = Vec3::new(center_x, 0.5, 15.0);
        puck.body.linear_velocity = Vec3::new(0.0, 0.0, -0.3);
        t.tick_until(500, attempt_over);
        assert_eq!(
            t.take_messages(red),
            [Received::Goal {
                team: Team::Red,
                scorer: None,
                assist: None,
            }]
        );
        let scoreboard = t.tick().unwrap();
        assert_eq!((scoreboard.red_score, scoreboard.blue_score), (1, 0));
        assert!(!scoreboard.game_over);

        // Blue's attempt runs out of time without a shot, and Red wins.
        t.tick_until(PAUSE_BETWEEN_ATTEMPTS, |t| attacking(t, Team::Blue));
        assert_eq!(t.take_chats(red), ["Last attempt for Blue"]);
        t.tick_until(ATTEMPT_TIME, attempt_over);
        assert_eq!(t.take_chats(red), ["Miss"]);
        assert!(t.tick().unwrap().game_over);

        // After the pause a new game starts, and the players are back on their teams.
        t.tick_until(PAUSE_BETWEEN_ATTEMPTS, |t| is_waiting(&t.mode.status));
        t.tick();
        assert_eq!(t.ctx().players.count_team_members(), (1, 1));
        assert_eq!(t.ctx().objects.pucks().count(), 1);
    }

    #[test]
    fn shootout_commands_are_handled_and_others_are_unknown() {
        let mut t = TestServer::new(ShootoutGameMode::new(CONFIG));
        let player = t.join("player");
        t.take_messages(player);

        t.chat(player, "/reset");
        t.chat(player, "/sp c");

        assert_eq!(
            t.take_chats(player),
            [
                "Please log in before using that command",
                "Unknown command /sp"
            ]
        );
    }
}

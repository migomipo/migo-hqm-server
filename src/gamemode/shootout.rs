use glam::Vec3;
use glamx::Rot3;
use reborrow::ReborrowMut;
use std::f32::consts::PI;
use tracing::info;

use crate::game::{PhysicsEvent, PlayerId, RulesState};
use crate::game::{PlayerIndex, Puck, ScoreboardValues, Team};
use crate::gamemode::util::{SpawnPoint, TeamSelection, get_spawnpoint};
use crate::gamemode::{ExitReason, GameMode, Server, ServerMut, ServerMutParts};

#[derive(Debug, Clone)]
enum ServerStatus {
    WaitingForGame { time: u32, time_until_start: u32 },
    Game(ShootoutGame),
}

#[derive(Debug, Clone)]
struct ShootoutGame {
    state: ShootoutAttemptState,
    game_state: ShootoutGameState,
}

#[derive(Debug, Clone)]
struct ShootoutGameState {
    config: ShootoutGameConfiguration,
    round: u32,
    team: Team,
    red_score: u32,
    blue_score: u32,
}

impl ShootoutGameState {
    fn finish_attempt(&mut self, goal_scored: bool) -> bool {
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
        let attempts = self.config.attempts.max(red_attempts_taken);
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
    fn before_tick(&mut self, mut server: ServerMut, game_state: &mut ShootoutGameState) -> bool {
        match self {
            Self::Attack(state) => {
                if let Some(end) = state.before_tick() {
                    *self = Self::Over(state.end_attempt(server.rb_mut(), game_state, end));
                }
                false
            }
            Self::Over(state) => match state.before_tick() {
                Some(OverTransition::StartNewGame) => true,
                Some(OverTransition::StartNextAttempt) => {
                    let attack = state.start_next_attempt(server.rb_mut(), game_state);
                    *self = Self::Attack(attack);
                    false
                }
                None => false,
            },
        }
    }

    fn tick(
        &mut self,
        mut server: ServerMut,
        attacking_team: Team,
        events: &[PhysicsEvent],
        game_state: &mut ShootoutGameState,
    ) {
        match self {
            Self::Attack(state) => {
                if let Some(end) = state.tick(server.rb_mut(), attacking_team, events) {
                    *self = Self::Over(state.end_attempt(server, game_state, end));
                }
            }
            Self::Over(_) => {}
        }
    }

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

#[derive(Debug, Clone, Copy)]
struct AttackTransition {
    time: u32,
    goal_scored: bool,
}

#[derive(Debug, Clone, Copy)]
enum OverTransition {
    StartNextAttempt,
    StartNewGame,
}

impl AttackState {
    fn setup_attempt(mut server: ServerMut, game_state: &ShootoutGameState) -> AttackState {
        let team = game_state.team;
        let defending_team = team.get_other_team();

        let remaining_attempts = game_state.config.attempts.saturating_sub(game_state.round);
        let msg = if remaining_attempts >= 2 {
            format!("{remaining_attempts} attempts left for {team}")
        } else if remaining_attempts == 1 {
            format!("Last attempt for {team}")
        } else {
            format!("Tie-breaker round for {team}")
        };
        server.add_server_chat_message(msg);
        server.state_mut().objects_mut().remove_all_pucks();

        let length = server.rink().length;
        let width = server.rink().width;

        let puck_pos = Vec3::new(width / 2.0, 1.0, length / 2.0);
        server
            .state_mut()
            .objects_mut()
            .spawn_puck(Puck::new(puck_pos, Rot3::IDENTITY));

        let mut red_players = vec![];
        let mut blue_players = vec![];

        for player in server.state().players().iter() {
            let player_index = player.id;
            if let Some(team) = player.team() {
                if team == Team::Red {
                    red_players.push(player_index);
                } else if team == Team::Blue {
                    blue_players.push(player_index);
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
        for (index, player_index) in attacking_players.into_iter().enumerate() {
            let mut pos = center_pos + attacking_rot * Vec3::new(0.0, 0.0, 3.0);
            if index > 0 {
                let dist = ((index / 2) + 1) as f32;

                let side = Vec3::new(-1.5 * dist, 0.0, 0.0);
                pos += attacking_rot * side;
            }
            server
                .state_mut()
                .spawn_skater(player_index, team, pos, attacking_rot, false);
        }
        for (index, player_index) in defending_players.into_iter().enumerate() {
            let mut pos = goalie_pos;
            if index > 0 {
                let dist = ((index / 2) + 1) as f32;

                let side = Vec3::new(-1.5 * dist, 0.0, 0.0);
                pos += defending_rot * side;
            }
            server.state_mut().spawn_skater(
                player_index,
                defending_team,
                pos,
                defending_rot,
                false,
            );
        }
        Self {
            time: 2000,
            progress: 0.0,
            no_more_touch: false,
        }
    }

    fn before_tick(&mut self) -> Option<AttackTransition> {
        self.time = self.time.saturating_sub(1);
        (self.time == 0).then_some(AttackTransition {
            time: 0,
            goal_scored: false,
        })
    }

    fn tick(
        &mut self,
        server: ServerMut,
        attacking_team: Team,
        events: &[PhysicsEvent],
    ) -> Option<AttackTransition> {
        for event in events {
            match event {
                PhysicsEvent::PuckEnteredNet { team: net_team, .. } => {
                    return Some(AttackTransition {
                        time: self.time,
                        goal_scored: net_team.get_other_team() == attacking_team,
                    });
                }
                PhysicsEvent::PuckPassedGoalLine { .. } => {
                    return Some(AttackTransition {
                        time: self.time,
                        goal_scored: false,
                    });
                }
                PhysicsEvent::PuckTouch { player, .. } => {
                    if let Some(touching_team) = server
                        .state()
                        .players()
                        .get(*player)
                        .and_then(|player| player.team())
                    {
                        if touching_team == attacking_team && self.no_more_touch {
                            return Some(AttackTransition {
                                time: self.time,
                                goal_scored: false,
                            });
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

        let center_pos = Vec3::new(server.rink().width / 2.0, 0.0, server.rink().length / 2.0);
        let normal = match attacking_team {
            Team::Red => Vec3::NEG_Z,
            Team::Blue => Vec3::Z,
        };
        let progress = {
            let state = server.state();
            let objects = state.objects();
            let puck = objects.get_puck(0)?;
            (puck.body.pos - center_pos).dot(normal)
        };
        if !self.no_more_touch && progress > self.progress {
            self.progress = progress;
            None
        } else if progress - self.progress < if self.no_more_touch { -5.0 } else { -0.5 } {
            Some(AttackTransition {
                time: self.time,
                goal_scored: false,
            })
        } else {
            None
        }
    }

    fn end_attempt(
        &mut self,
        mut server: ServerMut,
        game_state: &mut ShootoutGameState,
        end: AttackTransition,
    ) -> OverState {
        let game_over = game_state.finish_attempt(end.goal_scored);
        if end.goal_scored {
            server
                .state_mut()
                .add_goal_message(game_state.team, None, None);
        } else {
            server.add_server_chat_message("Miss");
        }

        OverState {
            timer_until_next_round: 500,
            time_at_end: end.time,
            goal_scored: end.goal_scored,
            game_over,
        }
    }
}

impl OverState {
    fn before_tick(&mut self) -> Option<OverTransition> {
        self.timer_until_next_round = self.timer_until_next_round.saturating_sub(1);
        (self.timer_until_next_round == 0).then_some(if self.game_over {
            OverTransition::StartNewGame
        } else {
            OverTransition::StartNextAttempt
        })
    }

    fn start_next_attempt(
        &self,
        server: ServerMut,
        game_state: &mut ShootoutGameState,
    ) -> AttackState {
        game_state.start_next_attempt();
        AttackState::setup_attempt(server, game_state)
    }
}

impl ShootoutGame {
    fn start(mut server: ServerMut, config: ShootoutGameConfiguration) -> Self {
        let game_state = ShootoutGameState {
            config: config.clone(),
            round: 0,
            team: Team::Red,
            red_score: 0,
            blue_score: 0,
        };
        let attack = AttackState::setup_attempt(server.rb_mut(), &game_state);
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
    fn before_tick(&mut self, mut server: ServerMut) {
        if self
            .state
            .before_tick(server.rb_mut(), &mut self.game_state)
        {
            server.new_game();
        }
    }

    fn tick(&mut self, server: ServerMut, events: &[PhysicsEvent]) {
        self.state
            .tick(server, self.game_state.team, events, &mut self.game_state);
    }
}

impl ShootoutGameMode {
    pub fn new(config: ShootoutGameConfiguration) -> Self {
        ShootoutGameMode {
            config,
            status: ServerStatus::WaitingForGame {
                time: 1000,
                time_until_start: 500,
            },
            teams: TeamSelection::new(1),
        }
    }

    fn start_game(&mut self, mut server: ServerMut) {
        let game = ShootoutGame::start(server.rb_mut(), self.config.clone());
        self.status = ServerStatus::Game(game);
    }

    fn update_players(&mut self, mut server: ServerMut) -> (usize, usize) {
        let ServerMutParts { state, rink, .. } = server.as_mut_parts();
        let rink = &*rink;
        self.teams.update(state, |team, _| {
            get_spawnpoint(rink, team, SpawnPoint::Bench)
        })
    }

    fn reset_game(&mut self, mut server: ServerMut, player_id: PlayerId) {
        if let Some(name) = server.require_admin(player_id) {
            info!("{} ({}) reset game", name, player_id);
            let msg = format!("Game reset by {name}");

            server.new_game();

            server.add_server_chat_message(msg);
        }
    }

    fn force_player_off_ice(
        &mut self,
        mut server: ServerMut,
        admin_player_id: PlayerId,
        force_player_index: PlayerIndex,
    ) {
        if let Some(admin_player_name) = server.require_admin(admin_player_id) {
            if let Some(force_player) = server.state().players().get_by_index(force_player_index) {
                let force_player_name = force_player.name();
                let force_player_id = force_player.id;
                if self
                    .teams
                    .force_spectate(server.state_mut(), force_player_id)
                {
                    let msg = format!("{force_player_name} forced off ice by {admin_player_name}");
                    info!(
                        "{} ({}) forced {} ({}) off ice",
                        admin_player_name, admin_player_id, force_player_name, force_player_index
                    );
                    server.add_server_chat_message(msg);
                }
            }
        }
    }
}

impl GameMode for ShootoutGameMode {
    fn before_tick(&mut self, mut server: ServerMut) {
        let (red_player_count, blue_player_count) = self.update_players(server.rb_mut());
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
                            self.start_game(server.rb_mut());
                        }
                    }
                } else {
                    *time = 1000;
                    *time_until_start = 500;
                }
            }
            ServerStatus::Game(game) => game.before_tick(server),
        }
    }

    fn after_tick(&mut self, server: ServerMut, events: &[PhysicsEvent]) -> ScoreboardValues {
        match self.status {
            ServerStatus::WaitingForGame { time, .. } => ScoreboardValues {
                rules_state: RulesState::default(),
                red_score: 0,
                blue_score: 0,
                period: if time == 0 { 1 } else { 0 },
                time,
                goal_message_timer: 0,
                game_over: false,
            },
            ServerStatus::Game(ref mut game) => {
                game.tick(server, events);
                game.scoreboard()
            }
        }
    }

    fn handle_command(&mut self, server: ServerMut, cmd: &str, arg: &str, player_id: PlayerId) {
        match cmd {
            "reset" | "resetgame" => {
                self.reset_game(server, player_id);
            }
            "fs" => {
                if let Ok(force_player_index) = arg.parse::<PlayerIndex>() {
                    self.force_player_off_ice(server, player_id, force_player_index);
                }
            }
            _ => {}
        }
    }

    fn game_started(&mut self, mut server: ServerMut) {
        self.status = ServerStatus::WaitingForGame {
            time: 1000,
            time_until_start: 500,
        };
        let rink = server.rink();
        let width = rink.width;
        let length = rink.length;

        let pos = Vec3::new(width / 2.0, 1.5, length / 2.0);
        let rot = Rot3::IDENTITY;
        server
            .state_mut()
            .objects_mut()
            .spawn_puck(Puck::new(pos, rot));
    }

    fn before_player_exit(&mut self, _server: ServerMut, player_id: PlayerId, _reason: ExitReason) {
        self.teams.remove_player(player_id);
    }

    fn server_list_team_size(&self) -> u32 {
        self.teams.team_max as u32
    }

    fn include_tick_in_recording(&self, _server: Server) -> bool {
        !matches!(self.status, ServerStatus::WaitingForGame { .. })
    }
}

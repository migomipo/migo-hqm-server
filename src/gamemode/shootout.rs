use nalgebra::{Point3, Rotation3, Vector3};
use reborrow::ReborrowMut;
use std::collections::HashMap;
use std::f32::consts::PI;

use tracing::info;

use crate::game::PhysicsEvent;
use crate::game::{PlayerIndex, PuckObject, ScoreboardValues, Team};
use crate::gamemode::util::{add_players, get_spawnpoint, SpawnPoint};
use crate::gamemode::{ExitReason, GameMode, InitialGameValues, ServerMut, ServerMutParts};

#[derive(Debug, Clone)]
enum ShootoutAttemptState {
    Attack { progress: f32 }, // Puck has been touched by attacker, but not touched by goalie, hit post or moved backwards
    NoMoreAttack { final_progress: f32 }, // Puck has moved backwards, hit the post or the goalie, but may still enter the net
    Over { timer: u32, goal_scored: bool }, // Attempt is over
}

#[derive(Debug, Clone)]
enum ShootoutStatus {
    WaitingForGame,
    Game {
        state: ShootoutAttemptState,
        round: u32,
        team: Team,
    },
}

pub struct ShootoutGameMode {
    attempts: u32,
    status: ShootoutStatus,
    paused: bool,
    team_switch_timer: HashMap<PlayerIndex, u32>,
    team_max: usize,
}

impl ShootoutGameMode {
    pub fn new(attempts: u32) -> Self {
        ShootoutGameMode {
            attempts,
            status: ShootoutStatus::WaitingForGame,
            paused: false,
            team_switch_timer: Default::default(),
            team_max: 1,
        }
    }

    fn init(&mut self, server: ServerMut) {
        self.start_next_attempt(server);
    }

    fn start_attempt(&mut self, mut server: ServerMut, round: u32, team: Team) {
        self.status = ShootoutStatus::Game {
            state: ShootoutAttemptState::Attack { progress: 0.0 },
            round,
            team,
        };

        let defending_team = team.get_other_team();

        let remaining_attempts = self.attempts.saturating_sub(round);
        let msg = if remaining_attempts >= 2 {
            format!("{} attempts left for {}", remaining_attempts, team)
        } else if remaining_attempts == 1 {
            format!("Last attempt for {}", team)
        } else {
            format!("Tie-breaker round for {}", team)
        };
        server.state_mut().add_server_chat_message(msg);

        let values = server.scoreboard_mut();
        values.time = 2000;
        values.goal_message_timer = 0;
        values.period = 1;
        server.state_mut().remove_all_pucks();

        let length = server.rink().length;
        let width = server.rink().width;

        let puck_pos = Point3::new(width / 2.0, 1.0, length / 2.0);
        server
            .state_mut()
            .spawn_puck(PuckObject::new(puck_pos, Rotation3::identity()));

        let mut red_players = vec![];
        let mut blue_players = vec![];

        for player in server.state().players().iter() {
            let player_index = player.id.index;
            if let Some(team) = player.team() {
                if team == Team::Red {
                    red_players.push(player_index);
                } else if team == Team::Blue {
                    blue_players.push(player_index);
                }
            }
        }

        let red_rot = Rotation3::identity();
        let blue_rot = Rotation3::from_euler_angles(0.0, PI, 0.0);

        let red_goalie_pos = Point3::new(width / 2.0, 1.5, length - 5.0);
        let blue_goalie_pos = Point3::new(width / 2.0, 1.5, 5.0);
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
        let center_pos = Point3::new(width / 2.0, 1.5, length / 2.0);
        for (index, player_index) in attacking_players.into_iter().enumerate() {
            let mut pos = center_pos + &attacking_rot * Vector3::new(0.0, 0.0, 3.0);
            if index > 0 {
                let dist = ((index / 2) + 1) as f32;

                let side = if index % 2 == 0 {
                    Vector3::new(-1.5 * dist, 0.0, 0.0)
                } else {
                    Vector3::new(-1.5 * dist, 0.0, 0.0)
                };
                pos += &attacking_rot * side;
            }
            server
                .state_mut()
                .spawn_skater(player_index, team, pos, attacking_rot.clone(), false);
        }
        for (index, player_index) in defending_players.into_iter().enumerate() {
            let mut pos = goalie_pos.clone();
            if index > 0 {
                let dist = ((index / 2) + 1) as f32;

                let side = if index % 2 == 0 {
                    Vector3::new(-1.5 * dist, 0.0, 0.0)
                } else {
                    Vector3::new(-1.5 * dist, 0.0, 0.0)
                };
                pos += &defending_rot * side;
            }
            server.state_mut().spawn_skater(
                player_index,
                defending_team,
                pos,
                defending_rot.clone(),
                false,
            );
        }
    }

    fn start_next_attempt(&mut self, server: ServerMut) {
        let (next_team, next_round) = match &self.status {
            ShootoutStatus::WaitingForGame => (Team::Red, 0),
            ShootoutStatus::Game { team, round, .. } => (
                team.get_other_team(),
                if *team == Team::Blue {
                    *round + 1
                } else {
                    *round
                },
            ),
        };

        self.start_attempt(server, next_round, next_team);
    }

    fn update_players(&mut self, mut server: ServerMut) {
        let ServerMutParts { state, rink, .. } = server.as_mut_parts();
        let rink = &*rink;
        add_players(
            state,
            self.team_max,
            &mut self.team_switch_timer,
            None,
            move |team, _| get_spawnpoint(rink, team, SpawnPoint::Bench),
            |_| {},
            |_, _| {},
        );
    }

    fn update_gameover(&mut self, mut server: ServerMut) {
        if let ShootoutStatus::Game { state, team, round } = &mut self.status {
            let is_attempt_over = if matches!(state, ShootoutAttemptState::Over { .. }) {
                1
            } else {
                0
            };
            let red_attempts_taken = *round + is_attempt_over;
            let blue_attempts_taken = *round
                + match team {
                    Team::Red => 0,
                    Team::Blue => is_attempt_over,
                };
            let attempts = self.attempts.max(red_attempts_taken);
            let remaining_red_attempts = attempts - red_attempts_taken;
            let remaining_blue_attempts = attempts - blue_attempts_taken;
            let values = server.scoreboard_mut();

            values.game_over =
                if let Some(difference) = values.red_score.checked_sub(values.blue_score) {
                    remaining_blue_attempts < difference
                } else if let Some(difference) = values.blue_score.checked_sub(values.red_score) {
                    remaining_red_attempts < difference
                } else {
                    false
                };
        }
    }

    fn end_attempt(&mut self, mut server: ServerMut, goal_scored: bool) {
        if let ShootoutStatus::Game { state, team, .. } = &mut self.status {
            let values = server.scoreboard_mut();
            if goal_scored {
                match team {
                    Team::Red => {
                        values.red_score += 1;
                    }
                    Team::Blue => {
                        values.blue_score += 1;
                    }
                }
                server.state_mut().add_goal_message(*team, None, None);
            } else {
                server.state_mut().add_server_chat_message("Miss");
            }
            *state = ShootoutAttemptState::Over {
                timer: 500,
                goal_scored,
            };
            self.update_gameover(server);
        }
    }

    fn reset_game(&mut self, mut server: ServerMut, player_index: PlayerIndex) {
        if let Some(player) = server.state().players().get(player_index) {
            if player.is_admin() {
                let name = player.name();
                info!("{} ({}) reset game", name, player_index);
                let msg = format!("Game reset by {}", name);

                server.new_game(self.get_initial_game_values());

                server.state_mut().add_server_chat_message(msg);
            } else {
                server.state_mut().admin_deny_message(player_index);
            }
        }
    }

    fn force_player_off_ice(
        &mut self,
        mut server: ServerMut,
        admin_player_index: PlayerIndex,
        force_player_index: PlayerIndex,
    ) {
        if let Some(player) = server.state().players().get(admin_player_index) {
            if player.is_admin() {
                let admin_player_name = player.name();

                if let Some(force_player) = server.state().players().get(force_player_index) {
                    let force_player_name = force_player.name();
                    if server.state_mut().move_to_spectator(force_player_index) {
                        let msg = format!(
                            "{} forced off ice by {}",
                            force_player_name, admin_player_name
                        );
                        info!(
                            "{} ({}) forced {} ({}) off ice",
                            admin_player_name,
                            admin_player_index,
                            force_player_name,
                            force_player_index
                        );
                        server.state_mut().add_server_chat_message(msg);
                        self.team_switch_timer.insert(force_player_index, 500);
                    }
                }
            } else {
                server.state_mut().admin_deny_message(admin_player_index);
                return;
            }
        }
    }

    fn set_score(
        &mut self,
        mut server: ServerMut,
        input_team: Team,
        input_score: u32,
        player_index: PlayerIndex,
    ) {
        if let Some(player) = server.state().players().get(player_index) {
            if player.is_admin() {
                match input_team {
                    Team::Red => {
                        let name = player.name();
                        server.scoreboard_mut().red_score = input_score;
                        info!(
                            "{} ({}) changed red score to {}",
                            name, player_index, input_score
                        );
                        let msg = format!("Red score changed by {}", name);
                        server.state_mut().add_server_chat_message(msg);
                    }
                    Team::Blue => {
                        let name = player.name();
                        server.scoreboard_mut().blue_score = input_score;
                        info!(
                            "{} ({}) changed blue score to {}",
                            name, player_index, input_score
                        );
                        let msg = format!("Blue score changed by {}", name);
                        server.state_mut().add_server_chat_message(msg);
                    }
                }
                self.update_gameover(server);
            } else {
                server.state_mut().admin_deny_message(player_index);
            }
        }
    }

    fn set_round(
        &mut self,
        mut server: ServerMut,
        input_team: Team,
        input_round: u32,
        player_index: PlayerIndex,
    ) {
        if input_round == 0 {
            return;
        }
        if let Some(player) = server.state().players().get(player_index) {
            if player.is_admin() {
                if let ShootoutStatus::Game {
                    state: _,
                    round,
                    team,
                } = &mut self.status
                {
                    *round = input_round - 1;
                    *team = input_team;
                    let name = player.name();

                    info!(
                        "{} ({}) changed round to {} for {}",
                        name, player_index, input_round, name
                    );
                    let msg = format!(
                        "Round changed to {} for {} by {}",
                        input_round, input_team, name
                    );
                    server.state_mut().add_server_chat_message(msg);
                }
                self.update_gameover(server);
            } else {
                server.state_mut().admin_deny_message(player_index);
            }
        }
    }

    fn redo_round(
        &mut self,
        mut server: ServerMut,
        input_team: Team,
        input_round: u32,
        player_index: PlayerIndex,
    ) {
        if input_round == 0 {
            return;
        }
        if let Some(player) = server.state().players().get(player_index) {
            if player.is_admin() {
                if let ShootoutStatus::Game {
                    state: _,
                    round,
                    team,
                } = &mut self.status
                {
                    *round = input_round - 1;
                    *team = input_team;
                }
                let name = player.name();
                info!(
                    "{} ({}) changed round to {} for {}",
                    name, player_index, input_round, input_team
                );
                let msg = format!(
                    "Round changed to {} for {} by {}",
                    input_round, input_team, name
                );
                server.state_mut().add_server_chat_message(msg);
                self.update_gameover(server.rb_mut());
                self.paused = false;
                if !server.scoreboard().game_over {
                    self.start_attempt(server.rb_mut(), input_round - 1, input_team);
                }
            } else {
                server.state_mut().admin_deny_message(player_index);
            }
        }
    }

    fn pause(&mut self, mut server: ServerMut, player_index: PlayerIndex) {
        if let Some(player) = server.state().players().get(player_index) {
            if player.is_admin() {
                self.paused = true;
                let name = player.name();

                info!("{} ({}) paused game", name, player_index);
                let msg = format!("Game paused by {}", name);
                server.state_mut().add_server_chat_message(msg);
            } else {
                server.state_mut().admin_deny_message(player_index);
            }
        }
    }

    fn unpause(&mut self, mut server: ServerMut, player_index: PlayerIndex) {
        if let Some(player) = server.state().players().get(player_index) {
            if player.is_admin() {
                self.paused = false;
                if let ShootoutStatus::Game {
                    state: ShootoutAttemptState::Over { timer, .. },
                    ..
                } = &mut self.status
                {
                    *timer = (*timer).max(200);
                }
                let name = player.name();
                info!("{} ({}) resumed game", name, player_index);
                let msg = format!("Game resumed by {}", name);

                server.state_mut().add_server_chat_message(msg);
            } else {
                server.state_mut().admin_deny_message(player_index);
            }
        }
    }
}

impl GameMode for ShootoutGameMode {
    fn before_tick(&mut self, server: ServerMut) {
        self.update_players(server);
    }

    fn after_tick(&mut self, mut server: ServerMut, events: &[PhysicsEvent]) {
        for event in events {
            match event {
                PhysicsEvent::PuckEnteredNet { team: net_team, .. } => {
                    let scoring_team = net_team.get_other_team();
                    if let ShootoutStatus::Game {
                        state,
                        team: attacking_team,
                        ..
                    } = &mut self.status
                    {
                        if let ShootoutAttemptState::Over { .. } = *state {
                            // Ignore
                        } else {
                            let is_goal = scoring_team == *attacking_team;
                            self.end_attempt(server.rb_mut(), is_goal);
                        }
                    }
                }
                PhysicsEvent::PuckPassedGoalLine { .. } => {
                    if let ShootoutStatus::Game { state, .. } = &mut self.status {
                        if let ShootoutAttemptState::Over { .. } = *state {
                            // Ignore
                        } else {
                            self.end_attempt(server.rb_mut(), false);
                        }
                    }
                }
                PhysicsEvent::PuckTouch { player, .. } => {
                    let player = *player;

                    if let Some(touching_team) = server
                        .state()
                        .players()
                        .get(player)
                        .and_then(|player| player.team())
                    {
                        if let ShootoutStatus::Game {
                            state,
                            team: attacking_team,
                            ..
                        } = &mut self.status
                        {
                            if touching_team == *attacking_team {
                                if let ShootoutAttemptState::NoMoreAttack { .. } = *state {
                                    self.end_attempt(server.rb_mut(), false);
                                }
                            } else {
                                if let ShootoutAttemptState::Attack { progress } = *state {
                                    *state = ShootoutAttemptState::NoMoreAttack {
                                        final_progress: progress,
                                    }
                                }
                            }
                        }
                    }
                }
                PhysicsEvent::PuckTouchedNet { team: net_team, .. } => {
                    if let ShootoutStatus::Game {
                        state,
                        team: attacking_team,
                        ..
                    } = &mut self.status
                    {
                        let team = net_team.get_other_team();
                        if team == *attacking_team {
                            if let ShootoutAttemptState::Attack { progress } = *state {
                                *state = ShootoutAttemptState::NoMoreAttack {
                                    final_progress: progress,
                                };
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        match &mut self.status {
            ShootoutStatus::WaitingForGame => {
                let (red_player_count, blue_player_count) =
                    server.state().players().count_team_members();
                let values = server.scoreboard_mut();
                if red_player_count > 0 && blue_player_count > 0 && !self.paused {
                    values.time = values.time.saturating_sub(1);
                    if values.time == 0 {
                        self.init(server);
                    }
                } else {
                    values.time = 1000;
                }
            }
            ShootoutStatus::Game { state, team, .. } => {
                if !self.paused {
                    if let ShootoutAttemptState::Over { timer, goal_scored } = state {
                        *timer = timer.saturating_sub(1);
                        let values = server.scoreboard_mut();
                        values.goal_message_timer = if *goal_scored { *timer } else { 0 };
                        if *timer == 0 {
                            if values.game_over {
                                server.new_game(self.get_initial_game_values());
                            } else {
                                self.start_next_attempt(server);
                            }
                        }
                    } else {
                        let values = server.scoreboard_mut();
                        values.time = values.time.saturating_sub(1);
                        if values.time == 0 {
                            values.time = 1; // A hack to avoid "Intermission" or "Game starting"
                            self.end_attempt(server, false);
                        } else {
                            if let Some(puck) = server.state().get_puck(0) {
                                let puck_pos = &puck.body.pos;
                                let center_pos = Point3::new(
                                    server.rink().width / 2.0,
                                    0.0,
                                    server.rink().length / 2.0,
                                );
                                let pos_diff = puck_pos - center_pos;
                                let normal = match *team {
                                    Team::Red => -Vector3::z(),
                                    Team::Blue => Vector3::z(),
                                };
                                let progress = pos_diff.dot(&normal);
                                if let ShootoutAttemptState::Attack {
                                    progress: current_progress,
                                } = state
                                {
                                    if progress > *current_progress {
                                        *current_progress = progress;
                                    } else if progress - *current_progress < -0.5 {
                                        // Too far back
                                        self.end_attempt(server, false);
                                    }
                                } else if let ShootoutAttemptState::NoMoreAttack {
                                    final_progress,
                                } = *state
                                {
                                    if progress - final_progress < -5.0 {
                                        self.end_attempt(server, false);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    fn handle_command(
        &mut self,
        server: ServerMut,
        cmd: &str,
        arg: &str,
        player_index: PlayerIndex,
    ) {
        match cmd {
            "reset" | "resetgame" => {
                self.reset_game(server, player_index);
            }
            "fs" => {
                if let Ok(force_player_index) = arg.parse::<PlayerIndex>() {
                    self.force_player_off_ice(server, player_index, force_player_index);
                }
            }
            "set" => {
                let args = arg.split(" ").collect::<Vec<&str>>();
                if args.len() >= 2 {
                    match args[0] {
                        "redscore" => {
                            if let Ok(input_score) = args[1].parse::<u32>() {
                                self.set_score(server, Team::Red, input_score, player_index);
                            }
                        }
                        "bluescore" => {
                            if let Ok(input_score) = args[1].parse::<u32>() {
                                self.set_score(server, Team::Blue, input_score, player_index);
                            }
                        }
                        "round" => {
                            if args.len() >= 3 {
                                let team = match args[1] {
                                    "r" | "R" => Some(Team::Red),
                                    "b" | "B" => Some(Team::Blue),
                                    _ => None,
                                };
                                let round = args[2].parse::<u32>();
                                if let (Some(team), Ok(round)) = (team, round) {
                                    self.set_round(server, team, round, player_index);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            "redo" => {
                let args = arg.split(" ").collect::<Vec<&str>>();
                if args.len() >= 2 {
                    let team = match args[0] {
                        "r" | "R" => Some(Team::Red),
                        "b" | "B" => Some(Team::Blue),
                        _ => None,
                    };
                    let round = args[1].parse::<u32>();
                    if let (Some(team), Ok(round)) = (team, round) {
                        self.redo_round(server, team, round, player_index);
                    }
                }
            }
            "pause" | "pausegame" => {
                self.pause(server, player_index);
            }
            "unpause" | "unpausegame" => {
                self.unpause(server, player_index);
            }
            _ => {}
        }
    }

    fn get_initial_game_values(&mut self) -> InitialGameValues {
        InitialGameValues {
            values: ScoreboardValues {
                time: 1000,
                ..Default::default()
            },
            puck_slots: 1,
        }
    }

    fn game_started(&mut self, _server: ServerMut) {
        self.status = ShootoutStatus::WaitingForGame;
    }

    fn before_player_exit(
        &mut self,
        _server: ServerMut,
        player_index: PlayerIndex,
        _reason: ExitReason,
    ) {
        self.team_switch_timer.remove(&player_index);
    }

    fn server_list_team_size(&self) -> u32 {
        self.team_max as u32
    }

    fn save_replay_data(&self, _server: ServerMut) -> bool {
        !matches!(self.status, ShootoutStatus::WaitingForGame)
    }
}

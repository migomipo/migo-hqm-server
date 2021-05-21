use migo_hqm_server::hqm_game::{HQMTeam, HQMGame, HQMGameObject, HQMPhysicsConfiguration};
use migo_hqm_server::hqm_server::{HQMServerBehaviour, HQMServer, HQMSpawnPoint};
use migo_hqm_server::hqm_simulate::HQMSimulationEvent;
use nalgebra::{Matrix3, Vector3, Rotation3, Point3};
use std::f32::consts::{PI};

use tracing::info;

enum HQMShootoutAttemptState {
    Start, // Puck has not been touched yet
    Attack, // Puck has been touched by attacker, but not touched by goalie, hit post or moved backwards
    NoMoreAttack, // Puck has moved backwards, hit the post or the goalie, but may still enter the net
    Over, // Attempt is over
}

enum HQMShootoutStatus {
    Pause,
    Game {
        state: HQMShootoutAttemptState,
        round: u32,
        team: HQMTeam
    },
    GameOver
}

pub struct HQMShootoutBehaviour {
    attempts: u32,
    status: HQMShootoutStatus,
    physics_config: HQMPhysicsConfiguration,
    team_max: usize
}

impl HQMShootoutBehaviour {
    pub fn new (attempts: u32, physics_config: HQMPhysicsConfiguration) -> Self {
        HQMShootoutBehaviour {
            attempts,
            status: HQMShootoutStatus::Pause,
            physics_config,
            team_max: 1
        }
    }

    fn init (& mut self, server: & mut HQMServer) {
        self.start_next_attempt(server);
    }

    fn start_next_attempt (& mut self, server: & mut HQMServer) {
        let (next_team, next_round) = match & self.status {
            HQMShootoutStatus::Pause => (HQMTeam::Red, 0),
            HQMShootoutStatus::Game { team, round, .. } => {
                (team.get_other_team(), if *team == HQMTeam::Blue {
                    *round + 1
                } else {
                    *round
                })
            }
            HQMShootoutStatus::GameOver => panic!()
        };

        let remaining_attempts = self.attempts.saturating_sub(next_round);
        if remaining_attempts >= 2 {
            let msg = format!("{} attempts left for {}", remaining_attempts, next_team);
            server.add_server_chat_message(&msg);
        } else if remaining_attempts == 1 {
            let msg = format!("Last attempt for {}", next_team);
            server.add_server_chat_message(&msg);
        } else {
            let msg = format!("Tie-breaker round for {}", next_team);
            server.add_server_chat_message(&msg);
        }

        let defending_team = next_team.get_other_team();

        let mut red_players = vec![];
        let mut blue_players = vec![];

        for (player_index, player) in server.players.iter().enumerate() {
            if player.is_some() {
                let team = server.game.world.get_skater_object(player_index).map(|x| x.team);
                if team == Some(HQMTeam::Red) {
                    red_players.push(player_index);
                } else if team == Some(HQMTeam::Blue) {
                    blue_players.push(player_index);
                }
            }
        }
        server.game.time = 1500;
        server.game.period = 1;
        server.game.is_intermission_goal = false;
        server.game.world.clear_pucks();

        let length = server.game.world.rink.length;
        let width = server.game.world.rink.width;

        let puck_pos = Point3::new (width / 2.0, 1.0, length / 2.0);
        server.game.world.create_puck_object(puck_pos, Matrix3::identity());

        let red_rot = Rotation3::identity();
        let blue_rot = Rotation3::from_euler_angles(0.0, PI, 0.0);

        let red_goalie_pos = Point3::new (width / 2.0, 1.5, length - 5.0);
        let blue_goalie_pos = Point3::new (width / 2.0, 1.5, 5.0);
        let (attacking_players,
            defending_players,
            attacking_rot,
            defending_rot,
            goalie_pos) =
            match next_team {
            HQMTeam::Red => (red_players,
                             blue_players,
                             red_rot,
                             blue_rot,
                             blue_goalie_pos),
            HQMTeam::Blue => (blue_players,
                              red_players,
                              blue_rot,
                              red_rot,
                              red_goalie_pos)
        };
        let center_pos = Point3::new (width / 2.0, 1.5, length / 2.0);
        for (index, player_index) in attacking_players.into_iter().enumerate() {
            let mut pos = center_pos + &attacking_rot * Vector3::new(0.0, 0.0, 3.0);
            if index > 0 {
                let dist = ((index / 2) + 1) as f32;

                let side = if index % 2 == 0 {
                    Vector3::new (-1.5 * dist, 0.0, 0.0)
                } else {
                    Vector3::new (-1.5 * dist, 0.0, 0.0)
                };
                pos += &attacking_rot * side;
            }
            server.move_to_team(player_index, next_team, pos, attacking_rot.clone ());
        }
        for (index, player_index) in defending_players.into_iter().enumerate() {
            let mut pos = goalie_pos.clone();
            if index > 0 {
                let dist = ((index / 2) + 1) as f32;

                let side = if index % 2 == 0 {
                    Vector3::new (-1.5 * dist, 0.0, 0.0)
                } else {
                    Vector3::new (-1.5 * dist, 0.0, 0.0)
                };
                pos += &defending_rot * side;
            }
            server.move_to_team(player_index, defending_team, pos, defending_rot.clone());
        }

        self.status = HQMShootoutStatus::Game {
            state: HQMShootoutAttemptState::Start,
            round: next_round,
            team: next_team
        }
    }

    fn update_players (& mut self, server: & mut HQMServer) {
        let mut spectating_players = vec![];
        let mut joining_red = vec![];
        let mut joining_blue = vec![];
        for (player_index, player) in server.players.iter_mut().enumerate() {
            if let Some(player) = player {
                let has_skater = server.game.world.has_skater(player_index);
                if has_skater && player.input.spectate() {
                    player.team_switch_timer = 500;
                    spectating_players.push((player_index, player.player_name.clone()))
                } else {
                    player.team_switch_timer = player.team_switch_timer.saturating_sub(1);
                }
                if !has_skater && player.team_switch_timer == 0 {
                    if player.input.join_red() {
                        joining_red.push((player_index, player.player_name.clone()));
                    } else if player.input.join_blue() {
                        joining_blue.push((player_index, player.player_name.clone()));
                    }
                }
            }
        }
        for (player_index, player_name) in spectating_players {
            info!("{} ({}) is spectating", player_name, player_index);
            server.move_to_spectator(player_index);
        }
        let (red_player_count, blue_player_count) = {
            let mut red_player_count = 0usize;
            let mut blue_player_count = 0usize;
            for p in server.game.world.objects.iter() {
                if let HQMGameObject::Player(player) = p {
                    if player.team == HQMTeam::Red {
                        red_player_count += 1;
                    } else if player.team == HQMTeam::Blue {
                        blue_player_count += 1;
                    }
                }
            }
            (red_player_count, blue_player_count)
        };
        let new_red_player_count = (red_player_count + joining_red.len()).min(self.team_max);
        let new_blue_player_count = (blue_player_count + joining_blue.len()).min(self.team_max);

        let num_joining_red = new_red_player_count.saturating_sub(red_player_count);
        let num_joining_blue = new_blue_player_count.saturating_sub(blue_player_count);
        for (player_index, player_name) in &joining_red[0..num_joining_red] {
            info!("{} ({}) has joined team {:?}", player_name, player_index, HQMTeam::Red);
            server.move_to_team_spawnpoint(*player_index, HQMTeam::Red, HQMSpawnPoint::Bench);
        }
        for (player_index, player_name) in &joining_blue[0..num_joining_blue] {
            info!("{} ({}) has joined team {:?}", player_name, player_index, HQMTeam::Blue);
            server.move_to_team_spawnpoint(*player_index, HQMTeam::Blue, HQMSpawnPoint::Bench);
        }
    }

    fn end_attempt (& mut self, server: & mut HQMServer, goal_scored: bool) {
        if let HQMShootoutStatus::Game { state, team, round } = & mut self.status {
            server.game.is_intermission_goal = goal_scored;
            server.game.time_break = 300;
            if goal_scored {
                match team {
                    HQMTeam::Red => {
                        server.game.red_score += 1;
                    }
                    HQMTeam::Blue => {
                        server.game.blue_score += 1;
                    }
                }
                server.add_goal_message(*team, None, None);
            } else {
                server.add_server_chat_message("Miss");
            }

            let red_attempts_taken = *round + 1;
            let blue_attempts_taken = *round + match team {
                HQMTeam::Red => 0,
                HQMTeam::Blue => 1
            };
            let attempts = self.attempts.max(red_attempts_taken);
            let remaining_red_attempts = attempts - red_attempts_taken;
            let remaining_blue_attempts = attempts - blue_attempts_taken;

            let game_over = if let Some(difference) = server.game.red_score.checked_sub(server.game.blue_score) {
                remaining_blue_attempts < difference
            } else if let Some(difference) = server.game.blue_score.checked_sub(server.game.red_score) {
                remaining_red_attempts < difference
            } else {
                false
            };
            if game_over {
                server.game.game_over = true;
                server.game.time_break = 500;
                self.status = HQMShootoutStatus::GameOver;
            } else {
                *state = HQMShootoutAttemptState::Over;
            }
        }
    }

}

impl HQMServerBehaviour for HQMShootoutBehaviour {
    fn before_tick(&mut self, server: &mut HQMServer) {
        self.update_players(server);
    }

    fn after_tick(&mut self, server: &mut HQMServer, events: &[HQMSimulationEvent]) {
        for event in events {
            match event {
                HQMSimulationEvent::PuckEnteredNet { team: scoring_team, .. } => {
                    if let HQMShootoutStatus::Game { state, team: attacking_team, .. } = & mut self.status {
                        if let HQMShootoutAttemptState::Start | HQMShootoutAttemptState::Attack | HQMShootoutAttemptState::NoMoreAttack = *state {
                            let is_goal = *scoring_team == *attacking_team;
                            self.end_attempt(server, is_goal);
                        }
                    }

                }
                HQMSimulationEvent::PuckPassedGoalLine { .. } => {
                    if let HQMShootoutStatus::Game { state, .. } = & mut self.status {
                        if let HQMShootoutAttemptState::Start | HQMShootoutAttemptState::Attack | HQMShootoutAttemptState::NoMoreAttack = *state {
                            self.end_attempt(server, false);
                        }
                    }
                }
                HQMSimulationEvent::PuckTouch { player, puck, .. } => {
                    let (player, puck) = (*player, *puck);
                    if let HQMGameObject::Player(skater) = & server.game.world.objects[player] {
                        let this_connected_player_index = skater.connected_player_index;
                        let touching_team = skater.team;

                        if let HQMGameObject::Puck(puck) = &mut server.game.world.objects[puck] {
                            puck.add_touch(this_connected_player_index, touching_team, server.game.time);

                            if let HQMShootoutStatus::Game { state, team: attacking_team, .. } = & mut self.status {
                                if touching_team == *attacking_team {
                                    if let HQMShootoutAttemptState::Start = *state {
                                        *state = HQMShootoutAttemptState::Attack;
                                    } else if let HQMShootoutAttemptState::NoMoreAttack = *state {
                                        self.end_attempt(server, false);
                                    }
                                } else {
                                    if let HQMShootoutAttemptState::Attack = *state {
                                        *state = HQMShootoutAttemptState::NoMoreAttack
                                    } else if let HQMShootoutAttemptState::Start = *state {
                                        self.end_attempt(server, false);
                                    }
                                }
                            }
                        }
                    }
                }
                HQMSimulationEvent::PuckTouchedNet { team, .. } => {
                    if let HQMShootoutStatus::Game { state, team: attacking_team, .. } = & mut self.status {
                        if *team == *attacking_team {
                            if let HQMShootoutAttemptState::Start | HQMShootoutAttemptState::Attack = *state {
                                *state = HQMShootoutAttemptState::NoMoreAttack;
                            }
                        }
                    }
                },
                _ => {}
            }
        }

        match & mut self.status {
            HQMShootoutStatus::Pause => {
                let (red_player_count, blue_player_count) = {
                    let mut red_player_count = 0usize;
                    let mut blue_player_count = 0usize;
                    for p in server.game.world.objects.iter() {
                        if let HQMGameObject::Player(player) = p {
                            if player.team == HQMTeam::Red {
                                red_player_count += 1;
                            } else if player.team == HQMTeam::Blue {
                                blue_player_count += 1;
                            }
                        }
                    }
                    (red_player_count, blue_player_count)
                };
                if red_player_count > 0 && blue_player_count > 0 {
                    server.game.time = server.game.time.saturating_sub(1);
                    if server.game.time == 0 {
                        self.init (server);
                    }
                } else {
                    server.game.time = 1000;
                }

            }
            HQMShootoutStatus::Game { state, team, .. } => {
                if let HQMShootoutAttemptState::Over = *state {
                    server.game.time_break = server.game.time_break.saturating_sub(1);
                    if server.game.time_break == 0 {
                        self.start_next_attempt(server);
                    }
                } else {

                    let speed = if let HQMGameObject::Puck(puck) = & server.game.world.objects[0] {
                        let puck_speed = &puck.body.linear_velocity;
                        let normal = match *team {
                            HQMTeam::Red => -Vector3::z(),
                            HQMTeam::Blue => Vector3::z()
                        };
                        puck_speed.dot(&normal)
                    } else {
                        0.0
                    };
                    if let HQMShootoutAttemptState::Attack = *state {
                        if speed < 0.0 {
                            *state = HQMShootoutAttemptState::NoMoreAttack;
                        }

                    }

                    server.game.time = server.game.time.saturating_sub(1);
                    if server.game.time == 0 {
                        server.game.time = 1; // A hack to avoid "Intermission" or "Game starting"
                        self.end_attempt(server, false);
                    }
                }
            }
            HQMShootoutStatus::GameOver => {
                server.game.time_break = server.game.time_break.saturating_sub(1);
                if server.game.time_break == 0 {
                    let new_game = self.create_game();
                    server.new_game(new_game);
                }
            }
        }


    }

    fn handle_command(&mut self, _server: &mut HQMServer, _cmd: &str, _arg: &str, _player_index: usize) {

    }

    fn create_game(&mut self) -> HQMGame {
        self.status = HQMShootoutStatus::Pause;
        let mut game = HQMGame::new(1, self.physics_config.clone());

        game.time = 1000;
        game
    }

    fn get_number_of_players(&self) -> u32 {
        self.team_max as u32
    }
}
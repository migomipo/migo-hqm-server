use nalgebra::{Matrix3, Point3, Rotation3};
use tracing::info;

use crate::hqm_game::{HQMGame, HQMGameObject, HQMPhysicsConfiguration, HQMTeam};
use crate::hqm_server::{HQMServer, HQMServerBehaviour};
use crate::hqm_simulate::HQMSimulationEvent;
use std::f32::consts::FRAC_PI_2;
use crate::hqm_simulate;
use num_integer::Integer;

enum HQMRussianStatus {
    Pause,
    InZone(HQMTeam),
    GoalScored(HQMTeam) ,
    GameOver
}

pub(crate) struct HQMRussianBehaviour {
    pub(crate) attempts: u32,
    pub(crate) attempts_completed: u32,
    pub(crate) physics_config: HQMPhysicsConfiguration,
    status: HQMRussianStatus
}

impl HQMRussianBehaviour {
    pub fn new (attempts: u32, physics_config: HQMPhysicsConfiguration) -> Self {
        HQMRussianBehaviour {
            attempts,
            attempts_completed: 0,
            physics_config,
            status: HQMRussianStatus::Pause
        }
    }

    fn update_players (& mut self, server: & mut HQMServer) {
        let mut spectating_players = vec![];
        let mut joining_red = vec![];
        let mut joining_blue = vec![];
        for (player_index, player) in server.players.iter_mut().enumerate() {
            if let Some(player) = player {
                if player.skater.is_some() && player.input.spectate() {
                    player.team_switch_timer = 500;
                    spectating_players.push((player_index, player.player_name.clone()))
                } else {
                    player.team_switch_timer = player.team_switch_timer.saturating_sub(1);
                }
                if player.skater.is_none() && player.team_switch_timer == 0 {
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
        let new_red_player_count = (red_player_count + joining_red.len()).min(server.config.team_max);
        let new_blue_player_count = (blue_player_count + joining_blue.len()).min(server.config.team_max);

        let num_joining_red = new_red_player_count.saturating_sub(red_player_count);
        let num_joining_blue = new_blue_player_count.saturating_sub(blue_player_count);
        let rot = Rotation3::from_euler_angles(0.0,3.0 * FRAC_PI_2,0.0);
        for (player_index, player_name) in &joining_red[0..num_joining_red] {
            let z = (server.game.world.rink.length/2.0) + 12.0;
            let pos = Point3::new (0.5, 2.0, z);
            info!("{} ({}) has joined team {:?}", player_name, player_index, HQMTeam::Red);
            server.move_to_team(*player_index, HQMTeam::Red, pos, rot.clone());
        }
        for (player_index, player_name) in &joining_blue[0..num_joining_blue] {
            let z = (server.game.world.rink.length/2.0) - 12.0;
            let pos = Point3::new (0.5, 2.0, z);
            info!("{} ({}) has joined team {:?}", player_name, player_index, HQMTeam::Blue);
            server.move_to_team(*player_index, HQMTeam::Blue, pos, rot.clone());
        }
    }

    fn place_puck_for_team (& mut self, game: & mut HQMGame, team: HQMTeam) {
        for object in game.world.objects.iter_mut() {
            if let HQMGameObject::Puck(_puck) = object {
                *object = HQMGameObject::None;
            }
        }

        let z = match team {
            HQMTeam::Red => 55.0,
            HQMTeam::Blue => 6.0
        };
        let puck_pos = Point3::new(game.world.rink.width / 2.0,0.5, z);

        game.world.create_puck_object(puck_pos, Matrix3::identity());

        self.status = HQMRussianStatus::InZone(team);
    }

    fn init (& mut self, server: & mut HQMServer) {
        server.game.period = 1;
        server.game.time = 2000;

        for object in server.game.world.objects.iter_mut() {
            if let HQMGameObject::Puck(_puck) = object {
                *object = HQMGameObject::None;
            }
        }

        let mut red_players = vec![];
        let mut blue_players = vec![];

        self.place_puck_for_team(& mut server.game, HQMTeam::Red);

        let objects = &server.game.world.objects;
        for (player_index, player) in server.players.iter().enumerate() {
            if let Some(player) = player {
                let team = player.skater.and_then(|i| match &objects[i] {
                    HQMGameObject::Player(skater) => { Some(skater.team)},
                    _ => None
                });
                if team == Some(HQMTeam::Red) {
                    red_players.push(player_index);
                } else if team == Some(HQMTeam::Blue) {
                    blue_players.push(player_index);
                }
            }
        }

        let rot = Rotation3::from_euler_angles(0.0,3.0 * FRAC_PI_2,0.0);
        for (index, player_index) in red_players.into_iter().enumerate() {
            let z = (server.game.world.rink.length/2.0) + (12.0 + index as f32);
            let pos = Point3::new (0.5, 2.0, z);
            server.move_to_team(player_index, HQMTeam::Red, pos, rot.clone());
        }
        for (index, player_index) in blue_players.into_iter().enumerate() {
            let z = (server.game.world.rink.length/2.0) - (12.0 + index as f32);
            let pos = Point3::new (0.5, 2.0, z);
            server.move_to_team(player_index, HQMTeam::Blue, pos, rot.clone());
        }

        let s = format!("Each team will get {} attempts", self.attempts);
        server.add_server_chat_message(&s);
    }

    fn attempt_completed (& mut self, server: & mut HQMServer) {
        self.attempts_completed += 1;

        let red_attempts = self.attempts_completed.div_ceil(&2);
        let blue_attempts = self.attempts_completed / 2;

        let (last_attempt_team, num_attempts_this_team) = if self.attempts_completed % 2 == 1 {
            (HQMTeam::Red, red_attempts)
        } else {
            (HQMTeam::Blue, blue_attempts)
        };

        let attempts_left = self.attempts.checked_sub(num_attempts_this_team);
        if let Some(attempts_left) = attempts_left {
            let s = format!("{} attempts left for {}", attempts_left, last_attempt_team);
            server.add_server_chat_message(&s);
            if attempts_left == 0 && server.game.red_score == server.game.blue_score {
                let s = "Tie breaker rounds will be added until someone wins";
                server.add_server_chat_message(&s);
            }
        }
        server.game.time = 2000;
    }

    fn check_ending (& mut self, game: & mut HQMGame) {
        let red_attempts = self.attempts_completed.div_ceil(&2);
        let blue_attempts = self.attempts_completed / 2;
        let attempts = self.attempts.max(red_attempts);
        let remaining_red_attempts = attempts - red_attempts;
        let remaining_blue_attempts = attempts - blue_attempts;

        let game_over = if game.red_score > game.blue_score {
            game.red_score - game.blue_score > remaining_blue_attempts
        } else if game.blue_score > game.red_score {
            game.blue_score - game.red_score > remaining_red_attempts
        } else {
            false
        };
        if game_over {
            self.status = HQMRussianStatus::GameOver;
            game.game_over = true;
            game.time_break = 500;
        }

    }

}

impl HQMServerBehaviour for HQMRussianBehaviour {
    fn before_tick(&mut self, server: &mut HQMServer) {
        self.update_players(server);
    }

    fn after_tick(&mut self, server: &mut HQMServer, events: &[HQMSimulationEvent]) {
        let (red_player_count, blue_player_count) = {
            let mut red_player_count = 0usize;
            let mut blue_player_count = 0usize;
            for p in server.game.world.objects.iter_mut() {
                if let HQMGameObject::Player(player) = p {
                    let line = if player.team == HQMTeam::Red {
                        red_player_count += 1;
                        &server.game.world.rink.red_lines_and_net.defensive_line
                    } else  {
                        blue_player_count += 1;
                        &server.game.world.rink.blue_lines_and_net.defensive_line
                    };

                    let p = &line.point;
                    let normal = &line.normal;
                    for collision_ball in player.collision_balls.iter_mut() {
                        let pos = &collision_ball.pos;
                        let radius = collision_ball.radius;
                        let overlap = (p - pos).dot (normal) + radius;
                        if overlap > 0.0 {
                            let mut new = normal.scale(overlap * 0.03125) - collision_ball.velocity.scale(0.25);
                            if new.dot(&normal) > 0.0 {
                                hqm_simulate::limit_rejection(& mut new, &normal, 0.01);

                                collision_ball.velocity += new;
                            }
                        }
                    }
                }
            }
            (red_player_count, blue_player_count)
        };

        if let HQMRussianStatus::Pause = self.status {
            if red_player_count > 0 && blue_player_count > 0 {
                server.game.time = server.game.time.saturating_sub(1);
                if server.game.time == 0 {
                    self.init (server);
                }
            } else {
                server.game.time = 1000;
            }
        } else if let HQMRussianStatus::GoalScored(team) = self.status {
            server.game.time_break = server.game.time_break.saturating_sub(1);
            if server.game.time_break == 0 {
                let other_team = team.get_other_team();
                server.game.is_intermission_goal = false;
                self.place_puck_for_team(& mut server.game, other_team);
                server.game.time = 2000;
            }
        } else if let HQMRussianStatus::GameOver = self.status {
            server.game.time_break = server.game.time_break.saturating_sub(1);
            if server.game.time_break == 0 {
                let new_game = self.create_game();
                server.new_game(new_game);
            }
        } else if let HQMRussianStatus::InZone(zone_team) = self.status {
            for event in events {
                match event {
                    HQMSimulationEvent::PuckEnteredNet { team, .. } => {
                        // Goal!
                        match *team {
                            HQMTeam::Red => {
                                server.game.red_score += 1;
                            }
                            HQMTeam::Blue => {
                                server.game.blue_score += 1;
                            }
                        };
                        self.status = HQMRussianStatus::GoalScored(*team);
                        server.game.time_break = 300;
                        server.game.is_intermission_goal = true;
                        server.add_goal_message(*team, None, None);
                        self.check_ending(& mut server.game);
                    }
                    HQMSimulationEvent::PuckTouch { puck, player } => {
                        if let HQMGameObject::Player(skater) = & server.game.world.objects[*player] {
                            let this_connected_player_index = skater.connected_player_index;
                            let touching_team = skater.team;
                            if let HQMGameObject::Puck(puck) = & mut server.game.world.objects[*puck] {
                                puck.add_touch(this_connected_player_index, touching_team, server.game.time);
                            }
                            if touching_team != zone_team {
                                self.status = HQMRussianStatus::InZone(touching_team);
                                self.attempt_completed(server);
                            }
                        }
                    }
                    HQMSimulationEvent::PuckEnteredOffensiveZone { team, .. } => {
                        let other_team = team.get_other_team();
                        if zone_team != other_team {
                            self.status = HQMRussianStatus::InZone(other_team);
                            self.attempt_completed(server);
                        }
                    }
                    HQMSimulationEvent::PuckLeftOffensiveZone { .. } => {
                        self.check_ending(& mut server.game);
                    }
                    _ => {}
                }
            }
            server.game.time = server.game.time.saturating_sub(1);
            if server.game.time == 0 {
                self.check_ending(& mut server.game);
                match self.status {
                    HQMRussianStatus::InZone(team) => {
                        let other_team = team.get_other_team();
                        self.place_puck_for_team(& mut server.game, other_team);
                        self.attempt_completed(server);
                    }
                    _ => {}
                }
            }

        }


    }

    fn handle_command(&mut self, _server: &mut HQMServer, _cmd: &str, _arg: &str, _player_index: usize) {

    }

    fn create_game(&mut self) -> HQMGame {
        self.status = HQMRussianStatus::Pause;
        self.attempts_completed = 0;
        let mut game = HQMGame::new(1, self.physics_config.clone());

        game.time = 1000;
        game
    }
}
use nalgebra::{Point3, Rotation3};
use std::collections::HashMap;
use tracing::info;

use migo_hqm_server::hqm_game::{
    HQMGame, HQMPhysicsConfiguration, HQMSkaterObjectRef, HQMSkaterObjectRefMut, HQMTeam,
};
use migo_hqm_server::hqm_server::{HQMServer, HQMServerBehaviour, HQMServerPlayerData};
use migo_hqm_server::hqm_simulate;
use migo_hqm_server::hqm_simulate::HQMSimulationEvent;
use std::f32::consts::FRAC_PI_2;
use std::rc::Rc;

enum HQMRussianStatus {
    Pause,
    Game {
        in_zone: HQMTeam,
        round: u32,
        goal_scored: bool,
    },
    GameOver,
}

pub(crate) struct HQMRussianBehaviour {
    attempts: u32,
    physics_config: HQMPhysicsConfiguration,
    blue_line_location: f32,
    status: HQMRussianStatus,
    team_switch_timer: HashMap<usize, u32>,
    team_max: usize,
    pub dual_control: bool,
}

impl HQMRussianBehaviour {
    pub fn new(
        attempts: u32,
        team_max: usize,
        physics_config: HQMPhysicsConfiguration,
        blue_line_location: f32,
        dual_control: bool,
    ) -> Self {
        HQMRussianBehaviour {
            attempts,
            physics_config,
            blue_line_location,
            status: HQMRussianStatus::Pause,
            team_switch_timer: Default::default(),
            team_max,
            dual_control,
        }
    }

    fn update_players(&mut self, server: &mut HQMServer) {
        let mut spectating_players = vec![];
        let mut joining_red = vec![];
        let mut joining_blue = vec![];
        for (player_index, player) in server.players.iter().enumerate() {
            if let Some(player) = player {
                self.team_switch_timer
                    .get_mut(&player_index)
                    .map(|x| *x = x.saturating_sub(1));
                if player.input.join_red() || player.input.join_blue() {
                    let has_skater = server.game.world.objects.has_skater(player_index)
                        || server.get_dual_control_player(player_index).is_some();
                    if !has_skater
                        && self
                            .team_switch_timer
                            .get(&player_index)
                            .map_or(true, |x| *x == 0)
                    {
                        if player.input.join_red() {
                            joining_red.push((player_index, player.player_name.clone()));
                        } else if player.input.join_blue() {
                            joining_blue.push((player_index, player.player_name.clone()));
                        }
                    }
                } else if player.input.spectate() {
                    let has_skater = server.game.world.objects.has_skater(player_index)
                        || server.get_dual_control_player(player_index).is_some();
                    if has_skater {
                        self.team_switch_timer.insert(player_index, 500);
                        spectating_players.push((player_index, player.player_name.clone()))
                    }
                }
            }
        }
        for (player_index, player_name) in spectating_players {
            info!("{} ({}) is spectating", player_name, player_index);
            if self.dual_control {
                server.remove_player_from_dual_control(player_index);
            } else {
                server.move_to_spectator(player_index);
            }
        }
        if !joining_red.is_empty() || !joining_blue.is_empty() {
            let (red_player_count, blue_player_count) = {
                let mut red_player_count = 0usize;
                let mut blue_player_count = 0usize;
                for HQMSkaterObjectRef { team, .. } in server.game.world.objects.get_skater_iter() {
                    if team == HQMTeam::Red {
                        red_player_count += 1;
                    } else if team == HQMTeam::Blue {
                        blue_player_count += 1;
                    }
                }
                (red_player_count, blue_player_count)
            };
            let mut new_red_player_count = red_player_count;
            let mut new_blue_player_count = blue_player_count;

            fn add_players(
                joining: Vec<(usize, Rc<String>)>,
                server: &mut HQMServer,
                team: HQMTeam,
                player_count: &mut usize,
                team_max: usize,
            ) {
                let mid_z = server.game.world.rink.length / 2.0;
                let z = match team {
                    HQMTeam::Red => mid_z + 12.0,
                    HQMTeam::Blue => mid_z - 12.0,
                };
                let pos = Point3::new(0.5, 2.0, z);
                let rot = Rotation3::from_euler_angles(0.0, 3.0 * FRAC_PI_2, 0.0);
                for (player_index, player_name) in joining {
                    if *player_count >= team_max {
                        break;
                    }

                    if server
                        .spawn_skater(player_index, team, pos.clone(), rot.clone())
                        .is_some()
                    {
                        info!(
                            "{} ({}) has joined team {:?}",
                            player_name, player_index, team
                        );
                        *player_count += 1
                    }
                }
            }
            fn add_players_dual_control(
                joining: Vec<(usize, Rc<String>)>,
                server: &mut HQMServer,
                team: HQMTeam,
                player_count: &mut usize,
                team_max: usize,
            ) {
                let mid_z = server.game.world.rink.length / 2.0;
                let z = match team {
                    HQMTeam::Red => mid_z + 12.0,
                    HQMTeam::Blue => mid_z - 12.0,
                };
                let pos = Point3::new(0.5, 2.0, z);
                let rot = Rotation3::from_euler_angles(0.0, 3.0 * FRAC_PI_2, 0.0);
                let mut current_empty = find_empty_dual_control(server, team);
                for (player_index, player_name) in joining {
                    match current_empty {
                        Some((index, movement @ Some(_), None)) => {
                            server.update_dual_control(index, movement, Some(player_index));
                            current_empty = find_empty_dual_control(server, team);
                        }
                        Some((index, None, stick @ Some(_))) => {
                            server.update_dual_control(index, Some(player_index), stick);
                            current_empty = find_empty_dual_control(server, team);
                        }
                        _ => {
                            if *player_count >= team_max {
                                break;
                            }

                            if let Some((dual_control_player_index, _)) = server
                                .spawn_dual_control_skater(
                                    team,
                                    pos.clone(),
                                    rot.clone(),
                                    Some(player_index),
                                    None,
                                )
                            {
                                info!(
                                    "{} ({}) has joined team {:?}",
                                    player_name, player_index, team
                                );
                                *player_count += 1;

                                current_empty =
                                    Some((dual_control_player_index, Some(player_index), None));
                            }
                        }
                    }
                }
            }

            if self.dual_control {
                add_players_dual_control(
                    joining_red,
                    server,
                    HQMTeam::Red,
                    &mut new_red_player_count,
                    self.team_max,
                );
                add_players_dual_control(
                    joining_blue,
                    server,
                    HQMTeam::Blue,
                    &mut new_blue_player_count,
                    self.team_max,
                );
            } else {
                add_players(
                    joining_red,
                    server,
                    HQMTeam::Red,
                    &mut new_red_player_count,
                    self.team_max,
                );
                add_players(
                    joining_blue,
                    server,
                    HQMTeam::Blue,
                    &mut new_blue_player_count,
                    self.team_max,
                );
            }
        }
    }

    fn place_puck_for_team(&mut self, server: &mut HQMServer, team: HQMTeam) {
        server.game.world.clear_pucks();

        let z = match team {
            HQMTeam::Red => 55.0,
            HQMTeam::Blue => 6.0,
        };
        let puck_pos = Point3::new(server.game.world.rink.width / 2.0, 0.5, z);

        server
            .game
            .world
            .create_puck_object(puck_pos, Rotation3::identity());

        self.fix_status(server, team);
    }

    fn fix_status(&mut self, server: &mut HQMServer, team: HQMTeam) {
        match &mut self.status {
            HQMRussianStatus::Pause => {
                self.status = HQMRussianStatus::Game {
                    in_zone: team,
                    round: 0,
                    goal_scored: false,
                };

                let remaining_attempts = self.attempts;
                if remaining_attempts >= 2 {
                    let msg = format!("{} attempts left for {}", remaining_attempts, team);
                    server.add_server_chat_message(msg);
                } else if remaining_attempts == 1 {
                    let msg = format!("Last attempt for {}", team);
                    server.add_server_chat_message(msg);
                } else {
                    let msg = format!("Tie-breaker round for {}", team);
                    server.add_server_chat_message(msg);
                }
            }
            HQMRussianStatus::Game { in_zone, round, .. } => {
                if *in_zone != team {
                    server.game.time = 2000;
                    *in_zone = team;
                    if team == HQMTeam::Red {
                        *round += 1;
                    }
                    let remaining_attempts = self.attempts.saturating_sub(*round);
                    if remaining_attempts >= 2 {
                        let msg = format!("{} attempts left for {}", remaining_attempts, team);
                        server.add_server_chat_message(msg);
                    } else if remaining_attempts == 1 {
                        let msg = format!("Last attempt for {}", team);
                        server.add_server_chat_message(msg);
                    } else {
                        let msg = format!("Tie-breaker round for {}", team);
                        server.add_server_chat_message(msg);
                    }
                }
            }
            HQMRussianStatus::GameOver => {}
        }
    }

    fn init(&mut self, server: &mut HQMServer) {
        server.game.period = 1;
        server.game.time = 2000;

        server.game.world.clear_pucks();

        let s = format!("Each team will get {} attempts", self.attempts);
        server.add_server_chat_message(s);

        let mut red_players = vec![];
        let mut blue_players = vec![];

        self.place_puck_for_team(server, HQMTeam::Red);

        for (player_index, player) in server.players.iter().enumerate() {
            if player.is_some() {
                let team = server
                    .game
                    .world
                    .objects
                    .get_skater_object_for_player(player_index)
                    .map(|x| x.team);
                if team == Some(HQMTeam::Red) {
                    red_players.push(player_index);
                } else if team == Some(HQMTeam::Blue) {
                    blue_players.push(player_index);
                }
            }
        }

        let rot = Rotation3::from_euler_angles(0.0, 3.0 * FRAC_PI_2, 0.0);
        for (index, player_index) in red_players.into_iter().enumerate() {
            let z = (server.game.world.rink.length / 2.0) + (12.0 + index as f32);
            let pos = Point3::new(0.5, 2.0, z);
            server.spawn_skater(player_index, HQMTeam::Red, pos, rot.clone());
        }
        for (index, player_index) in blue_players.into_iter().enumerate() {
            let z = (server.game.world.rink.length / 2.0) - (12.0 + index as f32);
            let pos = Point3::new(0.5, 2.0, z);
            server.spawn_skater(player_index, HQMTeam::Blue, pos, rot.clone());
        }
    }

    fn check_ending(&mut self, game: &mut HQMGame) {
        if let HQMRussianStatus::Game { in_zone, round, .. } = self.status {
            let red_attempts_taken = round + if in_zone == HQMTeam::Blue { 1 } else { 0 };
            let blue_attempts_taken = round;
            let attempts = self.attempts.max(red_attempts_taken);
            let remaining_red_attempts = attempts - red_attempts_taken;
            let remaining_blue_attempts = attempts - blue_attempts_taken;

            let game_over = if let Some(difference) = game.red_score.checked_sub(game.blue_score) {
                remaining_blue_attempts < difference
            } else if let Some(difference) = game.blue_score.checked_sub(game.red_score) {
                remaining_red_attempts < difference
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

    fn reset_game(&mut self, server: &mut HQMServer, player_index: usize) {
        if let Some(player) = server.players.get(player_index) {
            if player.is_admin {
                info!("{} ({}) reset game", player.player_name, player_index);
                let msg = format!("Game reset by {}", player.player_name);

                server.new_game(self.create_game());

                server.add_server_chat_message(msg);
            } else {
                server.admin_deny_message(player_index);
            }
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

            for HQMSkaterObjectRefMut {
                team,
                skater: player,
                ..
            } in server.game.world.objects.get_skater_iter_mut()
            {
                let line = if team == HQMTeam::Red {
                    red_player_count += 1;
                    &server.game.world.rink.red_lines_and_net.defensive_line
                } else {
                    blue_player_count += 1;
                    &server.game.world.rink.blue_lines_and_net.defensive_line
                };

                let p = &line.point;
                let normal = &line.normal;
                for collision_ball in player.collision_balls.iter_mut() {
                    let pos = &collision_ball.pos;
                    let radius = collision_ball.radius;
                    let overlap = (p - pos).dot(normal) + radius;
                    if overlap > 0.0 {
                        let mut new =
                            normal.scale(overlap * 0.03125) - collision_ball.velocity.scale(0.25);
                        if new.dot(&normal) > 0.0 {
                            hqm_simulate::limit_friction(&mut new, &normal, 0.01);

                            collision_ball.velocity += new;
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
                    self.init(server);
                }
            } else {
                server.game.time = 1000;
            }
        } else if let HQMRussianStatus::GameOver = self.status {
            server.game.time_break = server.game.time_break.saturating_sub(1);
            if server.game.time_break == 0 {
                let new_game = self.create_game();
                server.new_game(new_game);
            }
        } else if let HQMRussianStatus::Game {
            in_zone,
            round,
            goal_scored,
        } = self.status
        {
            if goal_scored {
                server.game.time_break = server.game.time_break.saturating_sub(1);
                if server.game.time_break == 0 {
                    server.game.is_intermission_goal = false;
                    self.place_puck_for_team(server, in_zone);
                    server.game.time = 2000;
                    self.status = HQMRussianStatus::Game {
                        in_zone,
                        round,
                        goal_scored: false,
                    };
                }
            } else {
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
                            self.status = HQMRussianStatus::Game {
                                in_zone,
                                round,
                                goal_scored: true,
                            };
                            server.game.time_break = 300;
                            server.game.is_intermission_goal = true;
                            server.add_goal_message(*team, None, None);
                            self.check_ending(&mut server.game);
                        }
                        HQMSimulationEvent::PuckTouch { puck, player, .. } => {
                            if let Some(HQMSkaterObjectRef {
                                connected_player_index: this_connected_player_index,
                                team: touching_team,
                                ..
                            }) = server.game.world.objects.get_skater(*player)
                            {
                                if let Some(puck) = server.game.world.objects.get_puck_mut(*puck) {
                                    puck.add_touch(
                                        this_connected_player_index,
                                        touching_team,
                                        server.game.time,
                                    );
                                }
                                self.fix_status(server, touching_team);
                            }
                        }
                        HQMSimulationEvent::PuckEnteredOffensiveZone { team, .. } => {
                            let other_team = team.get_other_team();
                            self.fix_status(server, other_team);
                        }
                        HQMSimulationEvent::PuckLeftOffensiveZone { .. } => {
                            self.check_ending(&mut server.game);
                        }
                        HQMSimulationEvent::PuckPassedGoalLine { .. } => {
                            self.check_ending(&mut server.game);
                        }
                        _ => {}
                    }
                }
                server.game.time = server.game.time.saturating_sub(1);
                if server.game.time == 0 {
                    self.check_ending(&mut server.game);
                    match self.status {
                        HQMRussianStatus::Game { in_zone, .. } => {
                            let other_team = in_zone.get_other_team();
                            self.place_puck_for_team(server, other_team);
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    fn handle_command(
        &mut self,
        server: &mut HQMServer,
        cmd: &str,
        _arg: &str,
        player_index: usize,
    ) {
        match cmd {
            "reset" | "resetgame" => {
                self.reset_game(server, player_index);
            }
            _ => {}
        }
    }

    fn create_game(&mut self) -> HQMGame {
        self.status = HQMRussianStatus::Pause;
        let mut game = HQMGame::new(1, self.physics_config.clone(), self.blue_line_location);

        game.time = 1000;
        game
    }

    fn before_player_exit(&mut self, _server: &mut HQMServer, player_index: usize) {
        self.team_switch_timer.remove(&player_index);
    }

    fn after_player_force_off(&mut self, _server: &mut HQMServer, player_index: usize) {
        self.team_switch_timer.insert(player_index, 500);
    }

    fn get_number_of_players(&self) -> u32 {
        self.team_max as u32
    }
}

fn find_empty_dual_control(
    server: &HQMServer,
    team: HQMTeam,
) -> Option<(usize, Option<usize>, Option<usize>)> {
    for (i, player) in server.players.iter().enumerate() {
        if let Some(player) = player {
            if let HQMServerPlayerData::DualControl { movement, stick } = player.data {
                if movement.is_none() || stick.is_none() {
                    if let Some(skater) = server.game.world.objects.get_skater_object_for_player(i)
                    {
                        if skater.team == team {
                            return Some((i, movement, stick));
                        }
                    }
                }
            }
        }
    }
    None
}

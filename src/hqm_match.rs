use nalgebra::{Matrix3, Point3, Vector3};
use tracing::info;

use crate::hqm_game::{HQMGame, HQMGameObject, HQMGameWorld, HQMPhysicsConfiguration, HQMSkaterHand, HQMTeam, HQMRinkFaceoffSpot, HQMRuleIndication};
use crate::hqm_server::{HQMIcingConfiguration, HQMOffsideConfiguration, HQMServer, HQMServerBehaviour, HQMSpawnPoint, HQMConnectedPlayer};
use crate::hqm_simulate::HQMSimulationEvent;
use std::collections::HashMap;

pub struct HQMMatchConfiguration {
    pub(crate) force_team_size_parity: bool,
    pub(crate) team_max: usize,
    pub(crate) time_period: u32,
    pub(crate) time_warmup: u32,
    pub(crate) time_break: u32,
    pub(crate) time_intermission: u32,
    pub(crate) mercy: u32,
    pub(crate) first_to: u32,
    pub(crate) offside: HQMOffsideConfiguration,
    pub(crate) icing: HQMIcingConfiguration,
    pub(crate) warmup_pucks: usize,
    pub(crate) physics_config: HQMPhysicsConfiguration,

    pub(crate) cheats_enabled: bool,

    pub(crate) spawn_point: HQMSpawnPoint,
}

pub struct HQMMatchBehaviour {
    config: HQMMatchConfiguration,
    paused: bool,
    next_faceoff_spot: HQMRinkFaceoffSpot,
    icing_status: HQMIcingStatus,
    offside_status: HQMOffsideStatus,
    preferred_positions: HashMap<usize, String>
}

#[derive(PartialEq, Debug, Clone)]
pub enum HQMIcingStatus {
    No,          // No icing
    NotTouched(HQMTeam, Point3<f32>),  // Puck has entered offensive half, but not reached the goal line
    Warning(HQMTeam, Point3<f32>),     // Puck has reached the goal line, delayed icing
    Icing(HQMTeam)       // Icing has been called
}

impl HQMIcingStatus {
    pub fn get_indication (& self) -> HQMRuleIndication {
        match self {
            HQMIcingStatus::Warning(_, _) => HQMRuleIndication::Warning,
            HQMIcingStatus::Icing(_) => HQMRuleIndication::Yes,
            _ => HQMRuleIndication::No
        }
    }
}

#[derive(PartialEq, Debug, Clone)]
pub enum HQMOffsideStatus {
    InNeutralZone,                // No offside
    InOffensiveZone(HQMTeam),              // No offside, puck in offensive zone
    Warning(HQMTeam, Point3<f32>, usize),  // Warning, puck entered offensive zone in an offside situation but not touched yet
    Offside(HQMTeam)                       // Offside has been called
}

impl HQMOffsideStatus {
    pub fn get_indication (& self) -> HQMRuleIndication {
        match self {
            HQMOffsideStatus::Warning(_, _, _) => HQMRuleIndication::Warning,
            HQMOffsideStatus::Offside(_) => HQMRuleIndication::Yes,
            _ => HQMRuleIndication::No
        }
    }
}

impl HQMMatchBehaviour {
    pub fn new (config: HQMMatchConfiguration) -> Self {
        HQMMatchBehaviour {
            config,
            paused: false,
            next_faceoff_spot: HQMRinkFaceoffSpot::Center,
            icing_status: HQMIcingStatus::No,
            offside_status: HQMOffsideStatus::InNeutralZone,
            preferred_positions: HashMap::new()
        }
    }

    pub fn do_faceoff(& mut self, server: & mut HQMServer){
        let positions = get_faceoff_positions(& server.players,
                                              & self.preferred_positions,
                                              & server.game.world.objects,
                                              &server.game.world.rink.allowed_positions);

        server.game.world.clear_pucks ();

        let next_faceoff_spot = server.game.world.rink.get_faceoff_spot(self.next_faceoff_spot).clone();

        let puck_pos = next_faceoff_spot.center_position + &(1.5f32*Vector3::y());

        server.game.world.create_puck_object(puck_pos, Matrix3::identity());

        for (player_index, (team, faceoff_position)) in positions {
            let (player_position, player_rotation) = match team {
                HQMTeam::Red => {
                    next_faceoff_spot.red_player_positions[&faceoff_position].clone()
                }
                HQMTeam::Blue => {
                    next_faceoff_spot.blue_player_positions[&faceoff_position].clone()
                }
            };
            server.move_to_team (player_index, team, player_position, player_rotation);
        }

        let rink = &server.game.world.rink;
        self.icing_status = HQMIcingStatus::No;
        self.offside_status = if rink.red_lines_and_net.offensive_line.point_past_middle_of_line(&puck_pos) {
            HQMOffsideStatus::InOffensiveZone(HQMTeam::Red)
        } else if rink.blue_lines_and_net.offensive_line.point_past_middle_of_line(&puck_pos) {
            HQMOffsideStatus::InOffensiveZone(HQMTeam::Blue)
        } else {
            HQMOffsideStatus::InNeutralZone
        };

    }

    pub fn call_goal(& mut self, server: & mut HQMServer, team: HQMTeam, puck: usize,
                     time_break: u32,
                     time_gameover: u32) {

        match team {
            HQMTeam::Red => {
                server.game.red_score += 1;
            }
            HQMTeam::Blue => {
                server.game.blue_score += 1;
            }
        };

        server.game.is_intermission_goal = true;
        self.next_faceoff_spot = HQMRinkFaceoffSpot::Center;

        let mut goal_scorer_index = None;
        let mut assist_index = None;

        if let HQMGameObject::Puck(this_puck) = & mut server.game.world.objects[puck] {
            for touch in this_puck.touches.iter() {
                if touch.team == team {
                    let player_index = touch.player_index;
                    if goal_scorer_index.is_none() {
                        goal_scorer_index = Some(player_index);
                    } else if assist_index.is_none() && Some(player_index) != goal_scorer_index {
                        assist_index = Some(player_index);
                        break;
                    }
                }
            }
        }

        self.goal_scored(server, team, goal_scorer_index, assist_index);

        if server.game.game_over {
            server.game.time_break = time_gameover;
        } else {
            server.game.time_break = time_break;
        }

        server.add_goal_message(team, goal_scorer_index, assist_index);
    }

    pub fn handle_events(& mut self, server: & mut HQMServer, events: &[HQMSimulationEvent], time_break: u32, time_intermission: u32,
                         offside: HQMOffsideConfiguration, icing: HQMIcingConfiguration) {
        if server.game.time_break > 0
            || server.game.time == 0
            || server.game.game_over
            || server.game.period == 0 {
            return;
        }

        for event in events {
            match event {
                HQMSimulationEvent::PuckEnteredNet {
                    team, puck
                } => {
                    let (team, puck) = (*team, *puck);
                    match &self.offside_status {
                        HQMOffsideStatus::Warning(offside_team, p, _) if *offside_team == team => {
                            let copy = p.clone();
                            self.call_offside(server, team, &copy, time_break);
                        }
                        HQMOffsideStatus::Offside(_) => {},
                        _ => {
                            self.call_goal(server, team, puck,
                                      time_break,
                                      time_intermission);
                        }
                    }
                },
                HQMSimulationEvent::PuckTouch {
                    player, puck
                } => {
                    let (player, puck) = (*player, *puck);
                    // Get connected player index from skater
                    if let HQMGameObject::Player(skater) = & server.game.world.objects[player] {
                        let this_connected_player_index = skater.connected_player_index;
                        let touching_team = skater.team;
                        let faceoff_position = skater.faceoff_position.clone();

                        if let HQMGameObject::Puck(puck) = & mut server.game.world.objects[puck] {
                            puck.add_touch(this_connected_player_index, touching_team, server.game.time);

                            let other_team = touching_team.get_other_team();

                            if let HQMOffsideStatus::Warning(team, p, i) = &self.offside_status {
                                if *team == touching_team {
                                    let pass_origin = if this_connected_player_index == *i {
                                        puck.body.pos.clone()
                                    } else {
                                        p.clone()
                                    };
                                    self.call_offside(server, touching_team, &pass_origin, time_break);
                                }
                                continue;

                            }
                            if let HQMIcingStatus::Warning(team, p) = &self.icing_status {
                                if touching_team != *team {
                                    if faceoff_position == "G" {
                                        self.icing_status = HQMIcingStatus::No;
                                        server.add_server_chat_message("Icing waved off");
                                    } else {
                                        let copy = p.clone();
                                        self.call_icing(server, other_team, &copy, time_break);
                                    }
                                } else {
                                    self.icing_status = HQMIcingStatus::No;
                                    server.add_server_chat_message("Icing waved off");
                                }
                            } else if let HQMIcingStatus::NotTouched (_, _) = self.icing_status {
                                self.icing_status = HQMIcingStatus::No;
                            }
                        }
                    }
                },
                HQMSimulationEvent::PuckEnteredOtherHalf {
                    team, puck
                } => {
                    let (team, puck) = (*team, *puck);
                    if let HQMGameObject::Puck(puck) = & server.game.world.objects[puck] {
                        if let Some(touch) = puck.touches.front() {
                            if team == touch.team && self.icing_status == HQMIcingStatus::No {
                                self.icing_status = HQMIcingStatus::NotTouched(team, touch.puck_pos.clone());
                            }
                        }
                    }
                },
                HQMSimulationEvent::PuckPassedGoalLine {
                    team, puck: _
                } => {
                    let team = *team;
                    if let HQMIcingStatus::NotTouched(icing_team, p) = &self.icing_status {
                        if team == *icing_team {
                            match icing {
                                HQMIcingConfiguration::Touch => {
                                    self.icing_status = HQMIcingStatus::Warning(team, p.clone());
                                    server.add_server_chat_message("Icing warning");
                                }
                                HQMIcingConfiguration::NoTouch => {
                                    let copy = p.clone();
                                    self.call_icing(server, team, &copy, time_break);
                                }
                                HQMIcingConfiguration::Off => {}
                            }
                        }
                    }
                },
                HQMSimulationEvent::PuckEnteredOffensiveZone {
                    team, puck
                } => {
                    let (team, puck) = (*team, *puck);
                    if self.offside_status == HQMOffsideStatus::InNeutralZone {
                        if let HQMGameObject::Puck(puck) = & server.game.world.objects[puck] {
                            if let Some(touch) = puck.touches.front() {
                                if team == touch.team &&
                                    has_players_in_offensive_zone(& server.game.world, team, Some(touch.player_index)) {
                                    match offside {
                                        HQMOffsideConfiguration::Delayed => {
                                            self.offside_status = HQMOffsideStatus::Warning(team, touch.puck_pos.clone(), touch.player_index);
                                            server.add_server_chat_message("Offside warning");
                                        }
                                        HQMOffsideConfiguration::Immediate => {
                                            let copy = touch.puck_pos.clone();
                                            self.call_offside(server, team, &copy, time_break);
                                        },
                                        HQMOffsideConfiguration::Off => {
                                            self.offside_status = HQMOffsideStatus::InOffensiveZone(team);
                                        }
                                    }
                                } else {
                                    self.offside_status = HQMOffsideStatus::InOffensiveZone(team);
                                }
                            } else {
                                self.offside_status = HQMOffsideStatus::InOffensiveZone(team);
                            }
                        }
                    }

                },
                HQMSimulationEvent::PuckLeftOffensiveZone {
                    team: _, puck: _
                } => {
                    if let HQMOffsideStatus::Warning(_, _, _) = self.offside_status {
                        server.add_server_chat_message("Offside waved off");
                    }
                    self.offside_status = HQMOffsideStatus::InNeutralZone;

                }
                _ => {}
            }
        }
        if let HQMOffsideStatus::Warning(team, _, _) = self.offside_status {
            if !has_players_in_offensive_zone(& server.game.world,team, None) {
                self.offside_status = HQMOffsideStatus::InOffensiveZone(team);
                server.add_server_chat_message("Offside waved off");
            }
        }
    }

    pub fn call_offside(& mut self, server: & mut HQMServer, team: HQMTeam, pass_origin: &Point3<f32>, time_break: u32) {
        self.next_faceoff_spot = server.game.world.rink.get_offside_faceoff_spot(pass_origin, team);
        server.game.time_break = time_break;
        self.offside_status = HQMOffsideStatus::Offside(team);
        server.add_server_chat_message("Offside");
    }

    pub fn call_icing(& mut self, server: & mut HQMServer, team: HQMTeam, pass_origin: &Point3<f32>, time_break: u32) {
        self.next_faceoff_spot = server.game.world.rink.get_icing_faceoff_spot(pass_origin, team);
        server.game.time_break = time_break;
        self.icing_status = HQMIcingStatus::Icing(team);
        server.add_server_chat_message("Icing");
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
            for p in server.game.world.objects.iter () {
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
        let mut new_red_player_count = (red_player_count + joining_red.len()).min(self.config.team_max);
        let mut new_blue_player_count = (blue_player_count + joining_blue.len()).min(self.config.team_max);

        if self.config.force_team_size_parity {
            if new_red_player_count > new_blue_player_count + 1 {
                new_red_player_count = new_blue_player_count + 1;
            } else if blue_player_count > new_red_player_count + 1 {
                new_blue_player_count = new_red_player_count + 1;
            }
        }
        let num_joining_red = new_red_player_count.saturating_sub(red_player_count);
        let num_joining_blue = new_blue_player_count.saturating_sub(blue_player_count);
        for (player_index, player_name) in &joining_red[0..num_joining_red] {
            info!("{} ({}) has joined team {:?}", player_name, player_index, HQMTeam::Red);
            server.move_to_team_spawnpoint(*player_index, HQMTeam::Red, self.config.spawn_point);
        }
        for (player_index, player_name) in &joining_blue[0..num_joining_blue] {
            info!("{} ({}) has joined team {:?}", player_name, player_index, HQMTeam::Blue);
            server.move_to_team_spawnpoint(*player_index, HQMTeam::Blue, self.config.spawn_point);
        }

        if server.game.period == 0 && server.game.time > 2000 && new_red_player_count > 0 && new_blue_player_count > 0 {
            server.game.time = 2000;
        }

    }

    pub(crate) fn set_team_parity(& mut self, server: & mut HQMServer, player_index: usize, rule:&str) {
        if let Some(player) = & server.players[player_index] {
            if player.is_admin{
                match rule {
                    "on" => {
                        self.config.force_team_size_parity = true;

                        info!("{} ({}) enabled team size parity",player.player_name, player_index);
                        let msg = format!("Team size parity enabled by {}", player.player_name);

                        server.add_server_chat_message(&msg);
                    },
                    "off" => {
                        self.config.force_team_size_parity = false;

                        info!("{} ({}) disabled team size parity",player.player_name, player_index);
                        let msg = format!("Team size parity disabled by {}", player.player_name);

                        server.add_server_chat_message(&msg);
                    }
                    _ => {}
                }
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    fn cheat_gravity (& mut self, server: & mut HQMServer, split: &[&str]) {
        if split.len() >= 2 {
            let gravity = split[1].parse::<f32>();
            if let Ok (gravity) = gravity {
                let converted_gravity = gravity/10000.0;
                self.config.physics_config.gravity = converted_gravity;
                server.game.world.physics_config.gravity = converted_gravity;
            }
        }
    }

    fn cheat_mass (& mut self, server: & mut HQMServer, split: &[&str]) {
        if split.len() >= 3 {
            let player = split[1].parse::<usize>().ok()
                .and_then(|x| server.players.get_mut(x).and_then(|x| x.as_mut()));
            let mass = split[2].parse::<f32>();
            if let Some(player) = player {
                if let Ok(mass) = mass {
                    player.mass = mass;
                    if let Some(skater_obj_index) = player.skater {
                        if let HQMGameObject::Player(skater) = & mut server.game.world.objects[skater_obj_index] {
                            for collision_ball in skater.collision_balls.iter_mut() {
                                collision_ball.mass = mass;
                            }
                        }
                    }
                }
            }
        }
    }

    pub(crate) fn cheat(& mut self, server: & mut HQMServer, player_index: usize, arg:&str) {
        if let Some(player) = & server.players[player_index] {

            if player.is_admin{
                let split: Vec<&str> = arg.split_whitespace().collect();
                if let Some(&command) = split.get(0) {
                    match command {
                        "mass" => {
                            self.cheat_mass(server, &split);
                        },
                        "gravity" => {
                            self.cheat_gravity(server, &split);
                        }
                        _ => {}
                    }
                }

            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    pub(crate) fn set_team_size(& mut self, server: & mut HQMServer, player_index: usize, size:&str) {
        if let Some(player) = & server.players[player_index] {
            if player.is_admin{
                if let Ok(new_num) = size.parse::<usize>() {
                    if new_num > 0 && new_num <= 15 {
                        self.config.team_max = new_num;

                        info!("{} ({}) set team size to {}",player.player_name, player_index, new_num);
                        let msg = format!("Team size set to {} by {}", new_num, player.player_name);

                        server.add_server_chat_message(&msg);
                    }
                }
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    pub(crate) fn set_icing_rule(& mut self, server: & mut HQMServer, player_index: usize, rule:&str) {
        if let Some(player) = & server.players[player_index] {
            if player.is_admin{
                match rule {
                    "on" | "touch" => {
                        self.config.icing = HQMIcingConfiguration::Touch;
                        info!("{} ({}) enabled touch icing",player.player_name, player_index);
                        let msg = format!("Touch icing enabled by {}",player.player_name);

                        server.add_server_chat_message(&msg);
                    },
                    "notouch" => {
                        self.config.icing = HQMIcingConfiguration::NoTouch;
                        info!("{} ({}) enabled no-touch icing",player.player_name, player_index);
                        let msg = format!("No-touch icing enabled by {}",player.player_name);

                        server.add_server_chat_message(&msg);
                    },
                    "off" => {
                        self.config.icing = HQMIcingConfiguration::Off;
                        info!("{} ({}) disabled icing",player.player_name, player_index);
                        let msg = format!("Icing disabled by {}",player.player_name);

                        server.add_server_chat_message(&msg);
                    }
                    _ => {}
                }
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    pub(crate) fn set_offside_rule(& mut self, server: & mut HQMServer, player_index: usize, rule:&str) {
        if let Some(player) = & server.players[player_index] {
            if player.is_admin{
                match rule {
                    "on" | "delayed" => {
                        self.config.offside = HQMOffsideConfiguration::Delayed;
                        info!("{} ({}) enabled offside", player.player_name, player_index);
                        let msg = format!("Offside enabled by {}",player.player_name);

                        server.add_server_chat_message(&msg);
                    },
                    "imm" | "immediate" => {
                        self.config.offside = HQMOffsideConfiguration::Immediate;
                        info!("{} ({}) enabled immediate offside", player.player_name, player_index);
                        let msg = format!("Immediate offside enabled by {}",player.player_name);

                        server.add_server_chat_message(&msg);
                    },
                    "off" => {
                        self.config.offside = HQMOffsideConfiguration::Off;
                        info!("{} ({}) disabled offside",player.player_name, player_index);
                        let msg = format!("Offside disabled by {}",player.player_name);

                        server.add_server_chat_message(&msg);
                    }
                    _ => {}
                }
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    pub(crate) fn set_first_to_rule(& mut self, server: & mut HQMServer, player_index: usize, size:&str) {
        if let Some(player) = & server.players[player_index] {
            if player.is_admin{
                if let Ok(new_num) = size.parse::<u32>() {
                    self.config.first_to = new_num;

                    if new_num > 0 {
                        info!("{} ({}) set first-to-goals rule to {} goals",player.player_name, player_index, new_num);
                        let msg = format!("First-to-goals rule set to {} goals by {}", new_num, player.player_name);
                        server.add_server_chat_message(&msg);
                    } else {
                        info!("{} ({}) disabled first-to-goals rule",player.player_name, player_index);
                        let msg = format!("First-to-goals rule disabled by {}", player.player_name);
                        server.add_server_chat_message(&msg);
                    }
                }
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    pub(crate) fn set_mercy_rule(& mut self, server: & mut HQMServer, player_index: usize, size:&str) {
        if let Some(player) = & server.players[player_index] {
            if player.is_admin{
                if let Ok(new_num) = size.parse::<u32>() {
                    self.config.mercy = new_num;

                    if new_num > 0 {
                        info!("{} ({}) set mercy rule to {} goals",player.player_name, player_index, new_num);
                        let msg = format!("Mercy rule set to {} goals by {}", new_num, player.player_name);
                        server.add_server_chat_message(&msg);
                    } else {
                        info!("{} ({}) disabled mercy rule",player.player_name, player_index);
                        let msg = format!("Mercy rule disabled by {}", player.player_name);
                        server.add_server_chat_message(&msg);
                    }
                }
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    pub(crate) fn faceoff (& mut self, server: & mut HQMServer, player_index: usize) {
        if !server.game.game_over {
            if let Some(player) = & server.players[player_index] {
                if player.is_admin{
                    server.game.time_break = 5*100;
                    self.paused = false; // Unpause if it's paused as well

                    let msg = format!("Faceoff initiated by {}",player.player_name);
                    info!("{} ({}) initiated faceoff",player.player_name, player_index);
                    server.add_server_chat_message(&msg);
                } else {
                    server.admin_deny_message(player_index);
                }
            }
        }
    }

    pub(crate) fn reset_game (& mut self, server: & mut HQMServer, player_index: usize) {
        if let Some(player) = & server.players[player_index] {
            if player.is_admin{
                info!("{} ({}) reset game",player.player_name, player_index);
                let msg = format!("Game reset by {}",player.player_name);

                server.new_game(self.create_game());

                server.add_server_chat_message(&msg);
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    pub(crate) fn start_game (& mut self, server: & mut HQMServer, player_index: usize) {
        if let Some(player) = & server.players[player_index] {
            if player.is_admin {
                if server.game.period == 0 && server.game.time > 1 {
                    info!("{} ({}) started game", player.player_name, player_index);
                    let msg = format!("Game started by {}", player.player_name);
                    self.paused = false;
                    server.game.time = 1;

                    server.add_server_chat_message(&msg);
                }
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    pub(crate) fn pause (& mut self, server: & mut HQMServer, player_index: usize) {
        if let Some(player) = & server.players[player_index] {
            if player.is_admin{
                self.paused=true;
                if server.game.time_break > 0 && server.game.time_break < self.config.time_break {
                    // If we're currently in a break, with very little time left,
                    // we reset the timer
                    server.game.time_break = self.config.time_break;
                }
                info!("{} ({}) paused game",player.player_name, player_index);
                let msg = format!("Game paused by {}",player.player_name);
                server.add_server_chat_message(&msg);
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    pub(crate) fn unpause (& mut self, server: & mut HQMServer, player_index: usize) {
        if let Some(player) = & server.players[player_index] {
            if player.is_admin{
                self.paused=false;
                info!("{} ({}) resumed game",player.player_name, player_index);
                let msg = format!("Game resumed by {}",player.player_name);

                server.add_server_chat_message(&msg);
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    pub(crate) fn set_clock (server: & mut HQMServer, input_minutes: u32, input_seconds: u32, player_index: usize) {
        if let Some(player) = & server.players[player_index] {
            if player.is_admin{
                server.game.time = (input_minutes * 60 * 100)+ (input_seconds * 100);

                info!("Clock set to {}:{} by {} ({})", input_minutes, input_seconds, player.player_name, player_index);
                let msg = format!("Clock set by {}", player.player_name);
                server.add_server_chat_message(&msg);
            } else {
                server.admin_deny_message(player_index);
            }
        }

    }

    pub(crate) fn set_score (server: & mut HQMServer, input_team: HQMTeam, input_score: u32, player_index: usize) {
        if let Some(player) = & server.players[player_index] {
            if player.is_admin{
                match input_team {
                    HQMTeam::Red =>{
                        server.game.red_score = input_score;

                        info!("{} ({}) changed red score to {}", player.player_name, player_index, input_score);
                        let msg = format!("Red score changed by {}",player.player_name);
                        server.add_server_chat_message(&msg);
                    },
                    HQMTeam::Blue =>{
                        server.game.blue_score = input_score;

                        info!("{} ({}) changed blue score to {}", player.player_name, player_index, input_score);
                        let msg = format!("Blue score changed by {}",player.player_name);
                        server.add_server_chat_message(&msg);
                    },
                }
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    pub(crate) fn set_period (server: & mut HQMServer, input_period: u32, player_index: usize) {
        if let Some(player) = & server.players[player_index] {
            if player.is_admin{

                server.game.period = input_period;

                info!("{} ({}) set period to {}", player.player_name, player_index, input_period);
                let msg = format!("Period set by {}",player.player_name);
                server.add_server_chat_message(&msg);

            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    pub(crate) fn set_preferred_faceoff_position(& mut self, server: & mut HQMServer, player_index: usize, input_position:&str) {
        let input_position = input_position.to_uppercase();
        if server.game.world.rink.allowed_positions.contains(& input_position) {
            if let Some(player) = & mut server.players[player_index] {
                info!("{} ({}) set position {}", player.player_name, player_index, input_position);
                let msg = format!("{} position {}", player.player_name, input_position);

                self.preferred_positions.insert(player_index, input_position);
                server.add_server_chat_message(&msg);

            }
        }
    }

    pub fn update_clock(& mut self, server: &mut HQMServer, period_length: u32, intermission_time: u32) {
        if server.game.time_break > 0 {
            server.game.time_break -= 1;
            if server.game.time_break == 0 {
                server.game.is_intermission_goal = false;
                if server.game.game_over {
                    server.new_game(self.create_game());
                } else {
                    if server.game.time == 0 {
                        server.game.time = period_length;
                    }

                    self.do_faceoff (server);
                }
            }
        } else {
            server.game.time = server.game.time.saturating_sub(1);
            if server.game.time == 0 {
                server.game.period += 1;
                server.game.time_break = intermission_time;
                self.next_faceoff_spot = HQMRinkFaceoffSpot::Center;
                self.period_over(server);
            }
        }

    }

    fn goal_scored(&mut self, server: &mut HQMServer, team: HQMTeam, _goal_index: Option<usize>, _assist_index: Option<usize>) {
        if server.game.time < 1000 {
            let time = server.game.time;
            let seconds = time / 100;
            let centi = time % 100;

            let s = format!("Goal scored with {}.{:02} seconds left", seconds, centi);
            server.add_server_chat_message(&s);

        }

        let (new_score, opponent_score) = match team {
            HQMTeam::Red => {
                (server.game.red_score, server.game.blue_score)
            }
            HQMTeam::Blue => {
                (server.game.blue_score, server.game.red_score)
            }
        };

        let game_over = if server.game.period > 3 && server.game.red_score != server.game.blue_score {
            true
        } else if self.config.mercy > 0 && new_score.saturating_sub(opponent_score) >= self.config.mercy {
            true
        } else if self.config.first_to > 0 && new_score >= self.config.first_to {
            true
        } else {
            false
        };
        if game_over {
            server.game.game_over = true;
        }
    }

    fn period_over(&mut self, server: &mut HQMServer) {
        if server.game.period > 3 && server.game.red_score != server.game.blue_score {
            server.game.game_over = true;
        }
    }

}

fn get_faceoff_positions (players: & [Option<HQMConnectedPlayer>],
                          preferred_positions: &HashMap<usize, String>,
                          objects: & [HQMGameObject],
                          allowed_positions: &[String]) -> HashMap<usize, (HQMTeam, String)> {
    let mut res = HashMap::new();

    let mut red_players= vec![];
    let mut blue_players = vec![];
    for (player_index, player) in players.iter().enumerate() {
        if let Some(player) = player {
            let team = player.skater.and_then(|i| match &objects[i] {
                HQMGameObject::Player(skater) => { Some(skater.team)},
                _ => None
            });
            if team == Some(HQMTeam::Red) {
                red_players.push((player_index, preferred_positions.get(&player_index)));
            } else if team == Some(HQMTeam::Blue) {
                blue_players.push((player_index, preferred_positions.get(&player_index)));
            }

        }
    }

    fn setup_position (positions: & mut HashMap<usize, (HQMTeam, String)>, players: &[(usize, Option<&String>)], allowed_positions: &[String], team: HQMTeam) {
        let mut available_positions = Vec::from(allowed_positions);

        // First, we try to give each player its preferred position
        for (player_index, player_position) in players.iter() {
            if let Some(player_position) = player_position {
                if let Some(x) = available_positions.iter().position(|x| *x == **player_position) {
                    let s = available_positions.remove(x);
                    positions.insert(*player_index, (team, s));
                }
            }
        }
        let c = String::from("C");
        // Some players did not get their preferred positions because they didn't have one,
        // or because it was already taken
        for (player_index, player_position) in players.iter() {
            if !positions.contains_key(player_index) {

                let s = if let Some(x) = available_positions.iter().position(|x| *x == c) {
                    // Someone needs to be C
                    available_positions.remove(x);
                    (team, c.clone())
                } else if !available_positions.is_empty() {
                    // Give out the remaining positions
                    let x = available_positions.remove(0);
                    (team, x)
                } else {
                    // Oh no, we're out of legal starting positions
                    if let Some(player_position) = player_position {
                        (team, (*player_position).clone())
                    } else {
                        (team, c.clone())
                    }
                };
                positions.insert(*player_index, s);
            }
        }
        if available_positions.contains(&c) && !players.is_empty() {
            positions.insert(players[0].0, (team, c.clone()));
        }
    }

    setup_position(& mut res, &red_players, allowed_positions, HQMTeam::Red);
    setup_position(& mut res, &blue_players, allowed_positions, HQMTeam::Blue);

    res
}



fn has_players_in_offensive_zone (world: & HQMGameWorld, team: HQMTeam, ignore_player: Option<usize>) -> bool {
    let line = match team {
        HQMTeam::Red => & world.rink.red_lines_and_net.offensive_line,
        HQMTeam::Blue => & world.rink.blue_lines_and_net.offensive_line,
    };

    for object in world.objects.iter() {
        if let HQMGameObject::Player(skater) = object {
            let player_index = skater.connected_player_index;
            if skater.team == team && ignore_player != Some(player_index) {
                let feet_pos = &skater.body.pos - (&skater.body.rot * Vector3::y().scale(skater.height));
                let dot = (&feet_pos - &line.point).dot (&line.normal);
                let leading_edge = -(line.width/2.0);
                if dot < leading_edge {
                    // Player is offside
                    return true;
                }
            }
        }
    }

    false
}

impl HQMServerBehaviour for HQMMatchBehaviour {
    fn before_tick(& mut self, server: &mut HQMServer) {
        self.update_players(server);
    }

    fn after_tick(& mut self, server: &mut HQMServer, events: &[HQMSimulationEvent]) {
        if !self.paused {
            self.handle_events(server, events, self.config.time_break*100,
                          self.config.time_intermission*100,
                          self.config.offside,
                          self.config.icing
            );

            self.update_clock(server, self.config.time_period*100, self.config.time_intermission*100);
        }
        server.game.icing_indication = self.icing_status.get_indication();
        server.game.offside_indication = self.offside_status.get_indication();
    }



    fn handle_command(&mut self, server: &mut HQMServer, command: &str, arg: &str, player_index: usize) {
        match command{
            "set" => {
                let args = arg.split(" ").collect::<Vec<&str>>();
                if args.len() > 1{
                    match args[0]{
                        "redscore" =>{
                            if let Ok (input_score) = args[1].parse::<u32>() {
                                Self::set_score(server, HQMTeam::Red,input_score,player_index);
                            }
                        },
                        "bluescore" =>{
                            if let Ok (input_score) = args[1].parse::<u32>() {
                                Self::set_score(server, HQMTeam::Blue,input_score,player_index);
                            }
                        },
                        "period" =>{
                            if let Ok (input_period) = args[1].parse::<u32>() {
                                Self::set_period(server, input_period,player_index);
                            }
                        },
                        "clock" =>{

                            let time_part_string = match args[1].parse::<String>(){
                                Ok(time_part_string) => time_part_string,
                                Err(_) => {return;}
                            };

                            let time_parts: Vec<&str> = time_part_string.split(':').collect();

                            if time_parts.len() >= 2{
                                if let (Ok(time_minutes), Ok(time_seconds)) = (time_parts[0].parse::<u32>(), time_parts[1].parse::<u32>()) {
                                    Self::set_clock(server, time_minutes,time_seconds, player_index);
                                }
                            }
                        },
                        "hand" =>{
                            match args[1]{
                                "left" =>{
                                    server.set_hand(HQMSkaterHand::Left, player_index);
                                },
                                "right" =>{
                                    server.set_hand(HQMSkaterHand::Right, player_index);
                                },
                                _=>{}
                            }
                        },
                        "icing" => {
                            if let Some(arg) = args.get(1) {
                                self.set_icing_rule(server, player_index, arg);
                            }
                        },
                        "offside" => {
                            if let Some(arg) = args.get(1) {
                                self.set_offside_rule(server, player_index, arg);
                            }
                        },
                        "mercy" => {
                            if let Some(arg) = args.get(1) {
                                self.set_mercy_rule(server, player_index, arg);
                            }
                        },
                        "first" => {
                            if let Some(arg) = args.get(1) {
                                self.set_first_to_rule(server, player_index, arg);
                            }
                        }
                        "teamsize" => {
                            if let Some(arg) = args.get(1) {
                                self.set_team_size(server, player_index, arg);
                            }
                        },
                        "teamparity" => {
                            if let Some(arg) = args.get(1) {
                                self.set_team_parity(server, player_index, arg);
                            }
                        },
                        "replay" => {
                            if let Some(arg) = args.get(1) {
                                server.set_replay(player_index, arg);
                            }
                        }
                        _ => {}
                    }
                }
            },
            "faceoff" => {
                self.faceoff(server, player_index);
            },
            "start" | "startgame" => {
                self.start_game(server, player_index);
            },
            "reset" | "resetgame" => {
                self.reset_game(server, player_index);
            },
            "pause" | "pausegame" => {
                self.pause(server, player_index);
            },
            "unpause" | "unpausegame" => {
                self.unpause(server, player_index);
            },
            "sp" | "setposition" => {
                self.set_preferred_faceoff_position(server, player_index, arg);
            },
            "icing" => {
                self.set_icing_rule (server, player_index, arg);
            },
            "offside" => {
                self.set_offside_rule (server, player_index, arg);
            },
            "rules" => {
                let offside_str = match self.config.offside {
                    HQMOffsideConfiguration::Off => "Offside disabled",
                    HQMOffsideConfiguration::Delayed => "Offside enabled",
                    HQMOffsideConfiguration::Immediate => "Immediate offside enabled"
                };
                let icing_str = match self.config.icing {
                    HQMIcingConfiguration::Off => "Icing disabled",
                    HQMIcingConfiguration::Touch => "Icing enabled",
                    HQMIcingConfiguration::NoTouch => "No-touch icing enabled"
                };
                let msg = format!("{}, {}", offside_str, icing_str);
                server.add_directed_server_chat_message(&msg, player_index);
            },
            "cheat" => {
                if self.config.cheats_enabled {
                    self.cheat(server, player_index, arg);
                }
            },
            _ => {}
        };

    }

    fn create_game(& mut self) -> HQMGame {
        self.paused = false;
        self.next_faceoff_spot = HQMRinkFaceoffSpot::Center;
        self.icing_status = HQMIcingStatus::No;
        self.offside_status = HQMOffsideStatus::InNeutralZone;

        let warmup_pucks = self.config.warmup_pucks;
        let mut game = HQMGame::new(warmup_pucks, self.config.physics_config.clone());
        let puck_line_start= game.world.rink.width / 2.0 - 0.4 * ((warmup_pucks - 1) as f32);

        for i in 0..warmup_pucks {
            let pos = Point3::new(puck_line_start + 0.8*(i as f32), 1.5, game.world.rink.length / 2.0);
            let rot = Matrix3::identity();
            game.world.create_puck_object(pos, rot);
        }
        game.time = self.config.time_warmup * 100;
        game
    }

    fn before_player_exit(&mut self, _server: &mut HQMServer, player_index: usize) {
        self.preferred_positions.remove(&player_index);
    }

    fn get_number_of_players(&self) -> u32 {
        self.config.team_max as u32
    }
}


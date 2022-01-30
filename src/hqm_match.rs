use nalgebra::{Point3, Rotation3, Vector3};
use tracing::info;

use migo_hqm_server::hqm_game::{
    HQMGame, HQMGameWorld, HQMPhysicsConfiguration, HQMPuck, HQMRinkFaceoffSpot, HQMRulesState,
    HQMTeam,
};
use migo_hqm_server::hqm_server::{
    HQMServer, HQMServerBehaviour, HQMServerPlayerData, HQMServerPlayerList, HQMSpawnPoint,
};
use migo_hqm_server::hqm_simulate::HQMSimulationEvent;
use std::collections::hash_map::Entry;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

pub struct HQMMatchConfiguration {
    pub team_max: usize,
    pub time_period: u32,
    pub time_warmup: u32,
    pub time_break: u32,
    pub time_intermission: u32,
    pub mercy: u32,
    pub first_to: u32,
    pub periods: u32,
    pub offside: HQMOffsideConfiguration,
    pub icing: HQMIcingConfiguration,
    pub warmup_pucks: usize,
    pub physics_config: HQMPhysicsConfiguration,
    pub blue_line_location: f32,
    pub cheats_enabled: bool,
    pub use_mph: bool,
    pub dual_control: bool,
    pub goal_replay: bool,

    pub spawn_point: HQMSpawnPoint,
}

#[derive(Eq, PartialEq, Debug, Copy, Clone)]
pub enum HQMIcingConfiguration {
    Off,
    Touch,
    NoTouch,
}

#[derive(Eq, PartialEq, Debug, Copy, Clone)]
pub enum HQMOffsideConfiguration {
    Off,
    Delayed,
    Immediate,
}

#[derive(Debug, Clone)]
pub struct HQMPuckTouch {
    pub player_index: usize,
    pub team: HQMTeam,
    pub puck_pos: Point3<f32>,
    pub puck_speed: f32,
    pub first_time: u32,
    pub last_time: u32,
}

pub struct HQMMatchBehaviour {
    pub config: HQMMatchConfiguration,
    pub paused: bool,
    pause_timer: u32,
    is_pause_goal: bool,
    next_faceoff_spot: HQMRinkFaceoffSpot,
    icing_status: HQMIcingStatus,
    offside_status: HQMOffsideStatus,
    preferred_positions: HashMap<usize, String>,
    team_switch_timer: HashMap<usize, u32>,
    started_as_goalie: Vec<usize>,
    faceoff_game_step: u32,
    step_where_period_ended: u32,
    too_late_printed_this_period: bool,
    start_next_replay: Option<(u32, u32, Option<usize>)>,
    puck_touches: HashMap<usize, VecDeque<HQMPuckTouch>>,
}

#[derive(PartialEq, Debug, Clone)]
pub enum HQMIcingStatus {
    No,                               // No icing
    NotTouched(HQMTeam, Point3<f32>), // Puck has entered offensive half, but not reached the goal line
    Warning(HQMTeam, Point3<f32>),    // Puck has reached the goal line, delayed icing
    Icing(HQMTeam),                   // Icing has been called
}

#[derive(PartialEq, Debug, Clone)]
pub enum HQMOffsideStatus {
    InNeutralZone,                        // No offside
    InOffensiveZone(HQMTeam),             // No offside, puck in offensive zone
    Warning(HQMTeam, Point3<f32>, usize), // Warning, puck entered offensive zone in an offside situation but not touched yet
    Offside(HQMTeam),                     // Offside has been called
}

impl HQMMatchBehaviour {
    pub fn new(config: HQMMatchConfiguration) -> Self {
        HQMMatchBehaviour {
            config,
            paused: false,
            pause_timer: 0,
            is_pause_goal: false,
            next_faceoff_spot: HQMRinkFaceoffSpot::Center,
            icing_status: HQMIcingStatus::No,
            offside_status: HQMOffsideStatus::InNeutralZone,
            preferred_positions: HashMap::new(),
            team_switch_timer: Default::default(),
            started_as_goalie: vec![],
            faceoff_game_step: 0,
            too_late_printed_this_period: false,
            step_where_period_ended: 0,
            start_next_replay: None,
            puck_touches: Default::default(),
        }
    }

    fn do_faceoff(&mut self, server: &mut HQMServer) {
        let positions = get_faceoff_positions(
            &server.players,
            &self.preferred_positions,
            &server.game.world,
        );

        server.game.world.clear_pucks();
        self.puck_touches.clear();

        let next_faceoff_spot = server
            .game
            .world
            .rink
            .get_faceoff_spot(self.next_faceoff_spot)
            .clone();

        let puck_pos = next_faceoff_spot.center_position + &(1.5f32 * Vector3::y());

        server
            .game
            .world
            .create_puck_object(puck_pos, Rotation3::identity());

        self.started_as_goalie.clear();
        for (player_index, (team, faceoff_position)) in positions {
            let (player_position, player_rotation) = match team {
                HQMTeam::Red => next_faceoff_spot.red_player_positions[&faceoff_position].clone(),
                HQMTeam::Blue => next_faceoff_spot.blue_player_positions[&faceoff_position].clone(),
            };
            server.spawn_skater(player_index, team, player_position, player_rotation);
            if faceoff_position == "G" {
                self.started_as_goalie.push(player_index);
            }
        }

        let rink = &server.game.world.rink;
        self.icing_status = HQMIcingStatus::No;
        self.offside_status = if rink
            .red_lines_and_net
            .offensive_line
            .point_past_middle_of_line(&puck_pos)
        {
            HQMOffsideStatus::InOffensiveZone(HQMTeam::Red)
        } else if rink
            .blue_lines_and_net
            .offensive_line
            .point_past_middle_of_line(&puck_pos)
        {
            HQMOffsideStatus::InOffensiveZone(HQMTeam::Blue)
        } else {
            HQMOffsideStatus::InNeutralZone
        };

        self.faceoff_game_step = server.game.game_step;
    }

    fn call_goal(&mut self, server: &mut HQMServer, team: HQMTeam, puck_index: usize) {
        let time_break = self.config.time_break * 100;
        let time_gameover = self.config.time_intermission * 100;

        match team {
            HQMTeam::Red => {
                server.game.red_score += 1;
            }
            HQMTeam::Blue => {
                server.game.blue_score += 1;
            }
        };

        self.next_faceoff_spot = HQMRinkFaceoffSpot::Center;

        let (
            goal_scorer_index,
            assist_index,
            puck_speed_across_line,
            puck_speed_from_stick,
            last_touch,
        ) = if let Some(this_puck) = server.game.world.objects.get_puck_mut(puck_index) {
            let mut goal_scorer_index = None;
            let mut assist_index = None;
            let mut goal_scorer_first_touch = 0;
            let mut puck_speed_from_stick = None;
            let mut last_touch = None;
            let puck_speed_across_line = this_puck.body.linear_velocity.norm();
            if let Some(touches) = self.puck_touches.get(&puck_index) {
                last_touch = touches.front().map(|x| x.player_index);

                for touch in touches.iter() {
                    if goal_scorer_index.is_none() {
                        if touch.team == team {
                            goal_scorer_index = Some(touch.player_index);
                            goal_scorer_first_touch = touch.first_time;
                            puck_speed_from_stick = Some(touch.puck_speed);
                        }
                    } else {
                        if touch.team == team {
                            if Some(touch.player_index) == goal_scorer_index {
                                goal_scorer_first_touch = touch.first_time;
                            } else {
                                // This is the first player on the scoring team that touched it apart from the goal scorer
                                // If more than 10 seconds passed between the goal scorer's first touch
                                // and this last touch, it doesn't count as an assist

                                let diff = touch.last_time.saturating_sub(goal_scorer_first_touch);

                                if diff <= 1000 {
                                    assist_index = Some(touch.player_index)
                                }
                                break;
                            }
                        }
                    }
                }
            }

            (
                goal_scorer_index,
                assist_index,
                puck_speed_across_line,
                puck_speed_from_stick,
                last_touch,
            )
        } else {
            return;
        };

        let (new_score, opponent_score) = match team {
            HQMTeam::Red => (server.game.red_score, server.game.blue_score),
            HQMTeam::Blue => (server.game.blue_score, server.game.red_score),
        };

        let game_over = if server.game.period > self.config.periods
            && server.game.red_score != server.game.blue_score
        {
            true
        } else if self.config.mercy > 0
            && new_score.saturating_sub(opponent_score) >= self.config.mercy
        {
            true
        } else if self.config.first_to > 0 && new_score >= self.config.first_to {
            true
        } else {
            false
        };

        server.add_goal_message(team, goal_scorer_index, assist_index);

        fn convert(puck_speed: f32, use_mph: bool) -> (f32, &'static str) {
            if use_mph {
                (puck_speed * 100f32 * 2.23693, "mph")
            } else {
                (puck_speed * 100f32 * 3.6, "km/h")
            }
        }

        let (puck_speed_across_line, puck_speed_unit) =
            convert(puck_speed_across_line, self.config.use_mph);

        let str1 = format!(
            "Goal scored, {:.1} {} across line",
            puck_speed_across_line, puck_speed_unit
        );

        let str2 = if let Some(puck_speed_from_stick) = puck_speed_from_stick {
            let (puck_speed, puck_speed_unit) = convert(puck_speed_from_stick, self.config.use_mph);
            format!(", {:.1} {} from stick", puck_speed, puck_speed_unit)
        } else {
            "".to_owned()
        };
        let s = format!("{}{}", str1, str2);

        server.add_server_chat_message(s);

        if server.game.time < 1000 {
            let time = server.game.time;
            let seconds = time / 100;
            let centi = time % 100;

            let s = format!("{}.{:02} seconds left", seconds, centi);
            server.add_server_chat_message(s);
        }

        if game_over {
            server.game.game_over = true;
        }

        if server.game.game_over {
            self.pause_timer = time_gameover;
        } else {
            self.pause_timer = time_break;
        }
        self.is_pause_goal = true;

        let gamestep = server.game.game_step;

        let force_view = goal_scorer_index.or(last_touch);

        if self.config.goal_replay {
            self.start_next_replay = Some((
                self.faceoff_game_step.max(gamestep - 600),
                gamestep + 200,
                force_view,
            ));
        }
    }

    fn handle_events_end_of_period(
        &mut self,
        server: &mut HQMServer,
        events: &[HQMSimulationEvent],
    ) {
        for event in events {
            if let HQMSimulationEvent::PuckEnteredNet { .. } = event {
                let time = server
                    .game
                    .game_step
                    .saturating_sub(self.step_where_period_ended);
                if time <= 300 && !self.too_late_printed_this_period {
                    let seconds = time / 100;
                    let centi = time % 100;
                    self.too_late_printed_this_period = true;
                    let s = format!("{}.{:02} seconds too late!", seconds, centi);

                    server.add_server_chat_message(s);
                }
            }
        }
    }

    fn handle_events(&mut self, server: &mut HQMServer, events: &[HQMSimulationEvent]) {
        let offside = self.config.offside;
        let icing = self.config.icing;
        for event in events {
            match event {
                HQMSimulationEvent::PuckEnteredNet { team, puck } => {
                    let (team, puck) = (*team, *puck);
                    match &self.offside_status {
                        HQMOffsideStatus::Warning(offside_team, p, _) if *offside_team == team => {
                            let copy = p.clone();
                            self.call_offside(server, team, &copy);
                        }
                        HQMOffsideStatus::Offside(_) => {}
                        _ => {
                            self.call_goal(server, team, puck);
                        }
                    }
                }
                HQMSimulationEvent::PuckTouch { player, puck, .. } => {
                    let (player, puck_index) = (*player, *puck);
                    // Get connected player index from skater
                    if let Some((player_index, touching_team, _)) =
                        server.players.get_from_object_index(player)
                    {
                        if let Some(puck) = server.game.world.objects.get_puck_mut(puck_index) {
                            add_touch(
                                puck,
                                self.puck_touches.entry(puck_index),
                                player_index,
                                touching_team,
                                server.game.time,
                            );

                            let other_team = touching_team.get_other_team();

                            if let HQMOffsideStatus::Warning(team, p, i) = &self.offside_status {
                                if *team == touching_team {
                                    let pass_origin = if player_index == *i {
                                        puck.body.pos.clone()
                                    } else {
                                        p.clone()
                                    };
                                    self.call_offside(server, touching_team, &pass_origin);
                                }
                                continue;
                            }
                            if let HQMIcingStatus::Warning(team, p) = &self.icing_status {
                                if touching_team != *team {
                                    if self.started_as_goalie.contains(&player_index) {
                                        self.icing_status = HQMIcingStatus::No;
                                        server.add_server_chat_message_str("Icing waved off");
                                    } else {
                                        let copy = p.clone();
                                        self.call_icing(server, other_team, &copy);
                                    }
                                } else {
                                    self.icing_status = HQMIcingStatus::No;
                                    server.add_server_chat_message_str("Icing waved off");
                                }
                            } else if let HQMIcingStatus::NotTouched(_, _) = self.icing_status {
                                self.icing_status = HQMIcingStatus::No;
                            }
                        }
                    }
                }
                HQMSimulationEvent::PuckEnteredOtherHalf { team, puck } => {
                    let (team, puck_index) = (*team, *puck);
                    if let Some(touch) = self.puck_touches.get(&puck_index).and_then(|x| x.front())
                    {
                        if team == touch.team && self.icing_status == HQMIcingStatus::No {
                            self.icing_status =
                                HQMIcingStatus::NotTouched(team, touch.puck_pos.clone());
                        }
                    }
                }
                HQMSimulationEvent::PuckPassedGoalLine { team, puck: _ } => {
                    let team = *team;
                    if let HQMIcingStatus::NotTouched(icing_team, p) = &self.icing_status {
                        if team == *icing_team {
                            match icing {
                                HQMIcingConfiguration::Touch => {
                                    self.icing_status = HQMIcingStatus::Warning(team, p.clone());
                                    server.add_server_chat_message_str("Icing warning");
                                }
                                HQMIcingConfiguration::NoTouch => {
                                    let copy = p.clone();
                                    self.call_icing(server, team, &copy);
                                }
                                HQMIcingConfiguration::Off => {}
                            }
                        }
                    }
                }
                HQMSimulationEvent::PuckEnteredOffensiveZone { team, puck } => {
                    let (team, puck_index) = (*team, *puck);
                    if self.offside_status == HQMOffsideStatus::InNeutralZone {
                        if let Some(touch) =
                            self.puck_touches.get(&puck_index).and_then(|x| x.front())
                        {
                            if team == touch.team
                                && has_players_in_offensive_zone(
                                    &server,
                                    team,
                                    Some(touch.player_index),
                                )
                            {
                                match offside {
                                    HQMOffsideConfiguration::Delayed => {
                                        self.offside_status = HQMOffsideStatus::Warning(
                                            team,
                                            touch.puck_pos.clone(),
                                            touch.player_index,
                                        );
                                        server.add_server_chat_message_str("Offside warning");
                                    }
                                    HQMOffsideConfiguration::Immediate => {
                                        let copy = touch.puck_pos.clone();
                                        self.call_offside(server, team, &copy);
                                    }
                                    HQMOffsideConfiguration::Off => {
                                        self.offside_status =
                                            HQMOffsideStatus::InOffensiveZone(team);
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
                HQMSimulationEvent::PuckLeftOffensiveZone { team: _, puck: _ } => {
                    if let HQMOffsideStatus::Warning(_, _, _) = self.offside_status {
                        server.add_server_chat_message_str("Offside waved off");
                    }
                    self.offside_status = HQMOffsideStatus::InNeutralZone;
                }
                _ => {}
            }
        }
        if let HQMOffsideStatus::Warning(team, _, _) = self.offside_status {
            if !has_players_in_offensive_zone(server, team, None) {
                self.offside_status = HQMOffsideStatus::InOffensiveZone(team);
                server.add_server_chat_message_str("Offside waved off");
            }
        }
    }

    fn call_offside(&mut self, server: &mut HQMServer, team: HQMTeam, pass_origin: &Point3<f32>) {
        let time_break = self.config.time_break * 100;

        self.next_faceoff_spot = server
            .game
            .world
            .rink
            .get_offside_faceoff_spot(pass_origin, team);
        self.pause_timer = time_break;
        self.offside_status = HQMOffsideStatus::Offside(team);
        server.add_server_chat_message_str("Offside");
    }

    fn call_icing(&mut self, server: &mut HQMServer, team: HQMTeam, pass_origin: &Point3<f32>) {
        let time_break = self.config.time_break * 100;

        self.next_faceoff_spot = server
            .game
            .world
            .rink
            .get_icing_faceoff_spot(pass_origin, team);
        self.pause_timer = time_break;
        self.icing_status = HQMIcingStatus::Icing(team);
        server.add_server_chat_message_str("Icing");
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
                    let has_skater = player.object.is_some()
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
                    let has_skater = player.object.is_some()
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
            if self.config.dual_control {
                server.remove_player_from_dual_control(player_index);
            } else {
                server.move_to_spectator(player_index);
            }
        }
        if !joining_red.is_empty() || !joining_blue.is_empty() {
            let (red_player_count, blue_player_count) = {
                let mut red_player_count = 0usize;
                let mut blue_player_count = 0usize;
                for player in server.players.iter() {
                    if let Some(player) = player {
                        if let Some((_, team)) = player.object {
                            if team == HQMTeam::Red {
                                red_player_count += 1;
                            } else if team == HQMTeam::Blue {
                                blue_player_count += 1;
                            }
                        }
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
                spawn_point: HQMSpawnPoint,
                player_count: &mut usize,
                team_max: usize,
                started_as_goalie: &mut Vec<usize>,
            ) {
                for (player_index, player_name) in joining {
                    if *player_count >= team_max {
                        break;
                    }

                    if server
                        .spawn_skater_at_spawnpoint(player_index, team, spawn_point)
                        .is_some()
                    {
                        info!(
                            "{} ({}) has joined team {:?}",
                            player_name, player_index, team
                        );
                        *player_count += 1;

                        if let Some(x) = started_as_goalie.iter().position(|x| *x == player_index) {
                            started_as_goalie.remove(x);
                        }
                    }
                }
            }
            fn add_players_dual_control(
                joining: Vec<(usize, Rc<String>)>,
                server: &mut HQMServer,
                team: HQMTeam,
                spawn_point: HQMSpawnPoint,
                player_count: &mut usize,
                team_max: usize,
                started_as_goalie: &mut Vec<usize>,
            ) {
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
                                .spawn_dual_control_skater_at_spawnpoint(
                                    team,
                                    spawn_point,
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

                                if let Some(x) = started_as_goalie
                                    .iter()
                                    .position(|x| *x == dual_control_player_index)
                                {
                                    started_as_goalie.remove(x);
                                }
                            }
                        }
                    }
                }
            }

            if self.config.dual_control {
                add_players_dual_control(
                    joining_red,
                    server,
                    HQMTeam::Red,
                    self.config.spawn_point,
                    &mut new_red_player_count,
                    self.config.team_max,
                    &mut self.started_as_goalie,
                );
                add_players_dual_control(
                    joining_blue,
                    server,
                    HQMTeam::Blue,
                    self.config.spawn_point,
                    &mut new_blue_player_count,
                    self.config.team_max,
                    &mut self.started_as_goalie,
                );
            } else {
                add_players(
                    joining_red,
                    server,
                    HQMTeam::Red,
                    self.config.spawn_point,
                    &mut new_red_player_count,
                    self.config.team_max,
                    &mut self.started_as_goalie,
                );
                add_players(
                    joining_blue,
                    server,
                    HQMTeam::Blue,
                    self.config.spawn_point,
                    &mut new_blue_player_count,
                    self.config.team_max,
                    &mut self.started_as_goalie,
                );
            }

            if server.game.period == 0
                && server.game.time > 2000
                && new_red_player_count > 0
                && new_blue_player_count > 0
            {
                server.game.time = 2000;
            }
        }
    }

    fn cheat_gravity(&mut self, server: &mut HQMServer, split: &[&str]) {
        if split.len() >= 2 {
            let gravity = split[1].parse::<f32>();
            if let Ok(gravity) = gravity {
                let converted_gravity = gravity / 10000.0;
                self.config.physics_config.gravity = converted_gravity;
                server.game.world.physics_config.gravity = converted_gravity;
            }
        }
    }

    fn cheat(&mut self, server: &mut HQMServer, player_index: usize, arg: &str) {
        if let Some(player) = server.players.get(player_index) {
            if player.is_admin {
                let split: Vec<&str> = arg.split_whitespace().collect();
                if let Some(&command) = split.get(0) {
                    match command {
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

    fn set_team_size(&mut self, server: &mut HQMServer, player_index: usize, size: &str) {
        if let Some(player) = server.players.get(player_index) {
            if player.is_admin {
                if let Ok(new_num) = size.parse::<usize>() {
                    if new_num > 0 && new_num <= 15 {
                        self.config.team_max = new_num;

                        info!(
                            "{} ({}) set team size to {}",
                            player.player_name, player_index, new_num
                        );
                        let msg = format!("Team size set to {} by {}", new_num, player.player_name);

                        server.add_server_chat_message(msg);
                    }
                }
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    fn set_icing_rule(&mut self, server: &mut HQMServer, player_index: usize, rule: &str) {
        if let Some(player) = server.players.get(player_index) {
            if player.is_admin {
                match rule {
                    "on" | "touch" => {
                        self.config.icing = HQMIcingConfiguration::Touch;
                        info!(
                            "{} ({}) enabled touch icing",
                            player.player_name, player_index
                        );
                        let msg = format!("Touch icing enabled by {}", player.player_name);

                        server.add_server_chat_message(msg);
                    }
                    "notouch" => {
                        self.config.icing = HQMIcingConfiguration::NoTouch;
                        info!(
                            "{} ({}) enabled no-touch icing",
                            player.player_name, player_index
                        );
                        let msg = format!("No-touch icing enabled by {}", player.player_name);

                        server.add_server_chat_message(msg);
                    }
                    "off" => {
                        self.config.icing = HQMIcingConfiguration::Off;
                        info!("{} ({}) disabled icing", player.player_name, player_index);
                        let msg = format!("Icing disabled by {}", player.player_name);

                        server.add_server_chat_message(msg);
                    }
                    _ => {}
                }
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    fn set_offside_rule(&mut self, server: &mut HQMServer, player_index: usize, rule: &str) {
        if let Some(player) = server.players.get(player_index) {
            if player.is_admin {
                match rule {
                    "on" | "delayed" => {
                        self.config.offside = HQMOffsideConfiguration::Delayed;
                        info!("{} ({}) enabled offside", player.player_name, player_index);
                        let msg = format!("Offside enabled by {}", player.player_name);

                        server.add_server_chat_message(msg);
                    }
                    "imm" | "immediate" => {
                        self.config.offside = HQMOffsideConfiguration::Immediate;
                        info!(
                            "{} ({}) enabled immediate offside",
                            player.player_name, player_index
                        );
                        let msg = format!("Immediate offside enabled by {}", player.player_name);

                        server.add_server_chat_message(msg);
                    }
                    "off" => {
                        self.config.offside = HQMOffsideConfiguration::Off;
                        info!("{} ({}) disabled offside", player.player_name, player_index);
                        let msg = format!("Offside disabled by {}", player.player_name);

                        server.add_server_chat_message(msg);
                    }
                    _ => {}
                }
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    fn set_goal_replay(&mut self, server: &mut HQMServer, player_index: usize, setting: &str) {
        if let Some(player) = server.players.get(player_index) {
            if player.is_admin {
                match setting {
                    "on" => {
                        self.config.goal_replay = true;
                        server.game.history_length = 850;
                        let msg = format!("Goal replays enabled by {}", player.player_name);
                        server.add_server_chat_message(msg);
                    }
                    "off" => {
                        self.config.goal_replay = false;
                        server.game.history_length = 0;
                        let msg = format!("Goal replays disabled by {}", player.player_name);
                        server.add_server_chat_message(msg);
                    }
                    _ => {}
                }
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    fn set_first_to_rule(&mut self, server: &mut HQMServer, player_index: usize, num: &str) {
        if let Some(player) = server.players.get(player_index) {
            if player.is_admin {
                let num = if num == "off" {
                    Some(0)
                } else {
                    num.parse::<u32>().ok()
                };
                if let Some(new_num) = num {
                    self.config.first_to = new_num;

                    if new_num > 0 {
                        info!(
                            "{} ({}) set first-to-goals rule to {} goals",
                            player.player_name, player_index, new_num
                        );
                        let msg = format!(
                            "First-to-goals rule set to {} goals by {}",
                            new_num, player.player_name
                        );
                        server.add_server_chat_message(msg);
                    } else {
                        info!(
                            "{} ({}) disabled first-to-goals rule",
                            player.player_name, player_index
                        );
                        let msg = format!("First-to-goals rule disabled by {}", player.player_name);
                        server.add_server_chat_message(msg);
                    }
                }
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    fn set_mercy_rule(&mut self, server: &mut HQMServer, player_index: usize, num: &str) {
        if let Some(player) = server.players.get(player_index) {
            if player.is_admin {
                let num = if num == "off" {
                    Some(0)
                } else {
                    num.parse::<u32>().ok()
                };
                if let Some(new_num) = num {
                    self.config.mercy = new_num;

                    if new_num > 0 {
                        info!(
                            "{} ({}) set mercy rule to {} goals",
                            player.player_name, player_index, new_num
                        );
                        let msg = format!(
                            "Mercy rule set to {} goals by {}",
                            new_num, player.player_name
                        );
                        server.add_server_chat_message(msg);
                    } else {
                        info!(
                            "{} ({}) disabled mercy rule",
                            player.player_name, player_index
                        );
                        let msg = format!("Mercy rule disabled by {}", player.player_name);
                        server.add_server_chat_message(msg);
                    }
                }
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    fn faceoff(&mut self, server: &mut HQMServer, player_index: usize) {
        if !server.game.game_over {
            if let Some(player) = server.players.get(player_index) {
                if player.is_admin {
                    self.pause_timer = 5 * 100;
                    self.paused = false; // Unpause if it's paused as well

                    let msg = format!("Faceoff initiated by {}", player.player_name);
                    info!(
                        "{} ({}) initiated faceoff",
                        player.player_name, player_index
                    );
                    server.add_server_chat_message(msg);
                } else {
                    server.admin_deny_message(player_index);
                }
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

    fn start_game(&mut self, server: &mut HQMServer, player_index: usize) {
        if let Some(player) = server.players.get(player_index) {
            if player.is_admin {
                if server.game.period == 0 && server.game.time > 1 {
                    info!("{} ({}) started game", player.player_name, player_index);
                    let msg = format!("Game started by {}", player.player_name);
                    self.paused = false;
                    server.game.time = 1;

                    server.add_server_chat_message(msg);
                }
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    fn pause(&mut self, server: &mut HQMServer, player_index: usize) {
        if let Some(player) = server.players.get(player_index) {
            if player.is_admin {
                self.paused = true;
                if self.pause_timer > 0 && self.pause_timer < self.config.time_break {
                    // If we're currently in a break, with very little time left,
                    // we reset the timer
                    self.pause_timer = self.pause_timer;
                }
                info!("{} ({}) paused game", player.player_name, player_index);
                let msg = format!("Game paused by {}", player.player_name);
                server.add_server_chat_message(msg);
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    fn unpause(&mut self, server: &mut HQMServer, player_index: usize) {
        if let Some(player) = server.players.get(player_index) {
            if player.is_admin {
                self.paused = false;
                info!("{} ({}) resumed game", player.player_name, player_index);
                let msg = format!("Game resumed by {}", player.player_name);

                server.add_server_chat_message(msg);
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    fn set_clock(
        server: &mut HQMServer,
        input_minutes: u32,
        input_seconds: u32,
        player_index: usize,
    ) {
        if let Some(player) = server.players.get(player_index) {
            if player.is_admin {
                server.game.time = (input_minutes * 60 * 100) + (input_seconds * 100);

                info!(
                    "Clock set to {}:{} by {} ({})",
                    input_minutes, input_seconds, player.player_name, player_index
                );
                let msg = format!("Clock set by {}", player.player_name);
                server.add_server_chat_message(msg);
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    fn set_score(
        server: &mut HQMServer,
        input_team: HQMTeam,
        input_score: u32,
        player_index: usize,
    ) {
        if let Some(player) = server.players.get(player_index) {
            if player.is_admin {
                match input_team {
                    HQMTeam::Red => {
                        server.game.red_score = input_score;

                        info!(
                            "{} ({}) changed red score to {}",
                            player.player_name, player_index, input_score
                        );
                        let msg = format!("Red score changed by {}", player.player_name);
                        server.add_server_chat_message(msg);
                    }
                    HQMTeam::Blue => {
                        server.game.blue_score = input_score;

                        info!(
                            "{} ({}) changed blue score to {}",
                            player.player_name, player_index, input_score
                        );
                        let msg = format!("Blue score changed by {}", player.player_name);
                        server.add_server_chat_message(msg);
                    }
                }
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    fn set_period(server: &mut HQMServer, input_period: u32, player_index: usize) {
        if let Some(player) = server.players.get(player_index) {
            if player.is_admin {
                server.game.period = input_period;

                info!(
                    "{} ({}) set period to {}",
                    player.player_name, player_index, input_period
                );
                let msg = format!("Period set by {}", player.player_name);
                server.add_server_chat_message(msg);
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    fn set_period_num(&mut self, server: &mut HQMServer, input_period: u32, player_index: usize) {
        if let Some(player) = server.players.get(player_index) {
            if player.is_admin {
                self.config.periods = input_period;

                info!(
                    "{} ({}) set number of periods to {}",
                    player.player_name, player_index, input_period
                );
                let msg = format!(
                    "Number of periods set to {} by {}",
                    input_period, player.player_name
                );
                server.add_server_chat_message(msg);
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    fn set_preferred_faceoff_position(
        &mut self,
        server: &mut HQMServer,
        player_index: usize,
        input_position: &str,
    ) {
        let input_position = input_position.to_uppercase();
        if server
            .game
            .world
            .rink
            .allowed_positions
            .contains(&input_position)
        {
            if let Some(player) = server.players.get(player_index) {
                info!(
                    "{} ({}) set position {}",
                    player.player_name, player_index, input_position
                );
                let msg = format!("{} position {}", player.player_name, input_position);

                self.preferred_positions
                    .insert(player_index, input_position);
                server.add_server_chat_message(msg);
            }
        }
    }

    fn update_clock(&mut self, server: &mut HQMServer) {
        let period_length = self.config.time_period * 100;
        let intermission_time = self.config.time_intermission * 100;

        if !self.paused {
            if self.pause_timer > 0 {
                self.pause_timer -= 1;
                if self.pause_timer == 0 {
                    self.is_pause_goal = false;
                    if server.game.game_over {
                        server.new_game(self.create_game());
                    } else {
                        if server.game.time == 0 {
                            server.game.time = period_length;
                        }

                        self.do_faceoff(server);
                    }
                }
            } else {
                server.game.time = server.game.time.saturating_sub(1);
                if server.game.time == 0 {
                    server.game.period += 1;
                    self.pause_timer = intermission_time;
                    self.is_pause_goal = false;
                    self.step_where_period_ended = server.game.game_step;
                    self.too_late_printed_this_period = false;
                    self.next_faceoff_spot = HQMRinkFaceoffSpot::Center;
                    if server.game.period > self.config.periods
                        && server.game.red_score != server.game.blue_score
                    {
                        server.game.game_over = true;
                    }
                }
            }
        }
        server.game.goal_message_timer = if self.is_pause_goal {
            self.pause_timer
        } else {
            0
        };
    }
}

fn get_faceoff_positions(
    players: &HQMServerPlayerList,
    preferred_positions: &HashMap<usize, String>,
    world: &HQMGameWorld,
) -> HashMap<usize, (HQMTeam, String)> {
    let allowed_positions = &world.rink.allowed_positions;
    let mut res = HashMap::new();

    let mut red_players = vec![];
    let mut blue_players = vec![];
    for (player_index, player) in players.iter().enumerate() {
        if let Some(player) = player {
            let team = player.object.map(|x| x.1);
            let i = match &player.data {
                HQMServerPlayerData::DualControl { movement, stick } => {
                    movement.or(*stick).unwrap_or(player_index)
                }
                _ => player_index,
            };
            let preferred_position = preferred_positions.get(&i).map(String::as_str);

            if team == Some(HQMTeam::Red) {
                red_players.push((player_index, preferred_position));
            } else if team == Some(HQMTeam::Blue) {
                blue_players.push((player_index, preferred_position));
            }
        }
    }

    setup_position(&mut res, &red_players, allowed_positions, HQMTeam::Red);
    setup_position(&mut res, &blue_players, allowed_positions, HQMTeam::Blue);

    res
}

fn has_players_in_offensive_zone(
    server: &HQMServer,
    team: HQMTeam,
    ignore_player: Option<usize>,
) -> bool {
    let line = match team {
        HQMTeam::Red => &server.game.world.rink.red_lines_and_net.offensive_line,
        HQMTeam::Blue => &server.game.world.rink.blue_lines_and_net.offensive_line,
    };

    for (player_index, player) in server.players.iter().enumerate() {
        if let Some(player) = player {
            if let Some((object_index, skater_team)) = player.object {
                if skater_team == team && ignore_player != Some(player_index) {
                    if let Some(skater) = server.game.world.objects.get_skater(object_index) {
                        let feet_pos = &skater.body.pos
                            - (&skater.body.rot * Vector3::y().scale(skater.height));
                        let dot = (&feet_pos - &line.point).dot(&line.normal);
                        let leading_edge = -(line.width / 2.0);
                        if dot < leading_edge {
                            // Player is offside
                            return true;
                        }
                    }
                }
            }
        }
    }

    false
}

impl HQMServerBehaviour for HQMMatchBehaviour {
    fn before_tick(&mut self, server: &mut HQMServer) {
        self.update_players(server);
    }

    fn after_tick(&mut self, server: &mut HQMServer, events: &[HQMSimulationEvent]) {
        if server.game.time == 0 && server.game.period > 1 {
            self.handle_events_end_of_period(server, events);
        } else if self.pause_timer > 0
            || server.game.time == 0
            || server.game.game_over
            || server.game.period == 0
            || self.paused
        {
            // Nothing
        } else {
            self.handle_events(server, events);

            let rules_state = if let HQMOffsideStatus::Offside(_) = self.offside_status {
                HQMRulesState::Offside
            } else if let HQMIcingStatus::Icing(_) = self.icing_status {
                HQMRulesState::Icing
            } else {
                let icing_warning = matches!(self.icing_status, HQMIcingStatus::Warning(_, _));
                let offside_warning =
                    matches!(self.offside_status, HQMOffsideStatus::Warning(_, _, _));
                HQMRulesState::Regular {
                    offside_warning,
                    icing_warning,
                }
            };

            server.game.rules_state = rules_state;
        }

        self.update_clock(server);

        if let Some((start_replay, end_replay, force_view)) = self.start_next_replay {
            if end_replay <= server.game.game_step {
                server.add_replay_to_queue(start_replay, end_replay, force_view);
                server.add_server_chat_message_str("Goal replay");
                self.start_next_replay = None;
            }
        }
    }

    fn handle_command(
        &mut self,
        server: &mut HQMServer,
        command: &str,
        arg: &str,
        player_index: usize,
    ) {
        match command {
            "set" => {
                let args = arg.split(" ").collect::<Vec<&str>>();
                if args.len() > 1 {
                    match args[0] {
                        "redscore" => {
                            if let Ok(input_score) = args[1].parse::<u32>() {
                                Self::set_score(server, HQMTeam::Red, input_score, player_index);
                            }
                        }
                        "bluescore" => {
                            if let Ok(input_score) = args[1].parse::<u32>() {
                                Self::set_score(server, HQMTeam::Blue, input_score, player_index);
                            }
                        }
                        "period" => {
                            if let Ok(input_period) = args[1].parse::<u32>() {
                                Self::set_period(server, input_period, player_index);
                            }
                        }
                        "periodnum" => {
                            if let Ok(input_period) = args[1].parse::<u32>() {
                                self.set_period_num(server, input_period, player_index);
                            }
                        }
                        "clock" => {
                            let time_part_string = match args[1].parse::<String>() {
                                Ok(time_part_string) => time_part_string,
                                Err(_) => {
                                    return;
                                }
                            };

                            let time_parts: Vec<&str> = time_part_string.split(':').collect();

                            if time_parts.len() >= 2 {
                                if let (Ok(time_minutes), Ok(time_seconds)) =
                                    (time_parts[0].parse::<u32>(), time_parts[1].parse::<u32>())
                                {
                                    Self::set_clock(
                                        server,
                                        time_minutes,
                                        time_seconds,
                                        player_index,
                                    );
                                }
                            }
                        }
                        "icing" => {
                            if let Some(arg) = args.get(1) {
                                self.set_icing_rule(server, player_index, arg);
                            }
                        }
                        "offside" => {
                            if let Some(arg) = args.get(1) {
                                self.set_offside_rule(server, player_index, arg);
                            }
                        }
                        "mercy" => {
                            if let Some(arg) = args.get(1) {
                                self.set_mercy_rule(server, player_index, arg);
                            }
                        }
                        "first" => {
                            if let Some(arg) = args.get(1) {
                                self.set_first_to_rule(server, player_index, arg);
                            }
                        }
                        "teamsize" => {
                            if let Some(arg) = args.get(1) {
                                self.set_team_size(server, player_index, arg);
                            }
                        }
                        "replay" => {
                            if let Some(arg) = args.get(1) {
                                server.set_replay(player_index, arg);
                            }
                        }
                        "goalreplay" => {
                            if let Some(arg) = args.get(1) {
                                self.set_goal_replay(server, player_index, arg);
                            }
                        }
                        _ => {}
                    }
                }
            }
            "faceoff" => {
                self.faceoff(server, player_index);
            }
            "start" | "startgame" => {
                self.start_game(server, player_index);
            }
            "reset" | "resetgame" => {
                self.reset_game(server, player_index);
            }
            "pause" | "pausegame" => {
                self.pause(server, player_index);
            }
            "unpause" | "unpausegame" => {
                self.unpause(server, player_index);
            }
            "sp" | "setposition" => {
                self.set_preferred_faceoff_position(server, player_index, arg);
            }
            "icing" => {
                self.set_icing_rule(server, player_index, arg);
            }
            "offside" => {
                self.set_offside_rule(server, player_index, arg);
            }
            "rules" => {
                let offside_str = match self.config.offside {
                    HQMOffsideConfiguration::Off => "Offside disabled",
                    HQMOffsideConfiguration::Delayed => "Offside enabled",
                    HQMOffsideConfiguration::Immediate => "Immediate offside enabled",
                };
                let icing_str = match self.config.icing {
                    HQMIcingConfiguration::Off => "Icing disabled",
                    HQMIcingConfiguration::Touch => "Icing enabled",
                    HQMIcingConfiguration::NoTouch => "No-touch icing enabled",
                };
                let msg = format!("{}, {}", offside_str, icing_str);
                server.add_directed_server_chat_message(msg, player_index);
                if self.config.mercy > 0 {
                    let msg = format!("Mercy rule when team leads by {} goals", self.config.mercy);
                    server.add_directed_server_chat_message(msg, player_index);
                }
                if self.config.first_to > 0 {
                    let msg = format!("Game ends when team scores {} goals", self.config.first_to);
                    server.add_directed_server_chat_message(msg, player_index);
                }
            }
            "cheat" => {
                if self.config.cheats_enabled {
                    self.cheat(server, player_index, arg);
                }
            }
            _ => {}
        };
    }

    fn create_game(&mut self) -> HQMGame {
        self.paused = false;
        self.next_faceoff_spot = HQMRinkFaceoffSpot::Center;
        self.icing_status = HQMIcingStatus::No;
        self.offside_status = HQMOffsideStatus::InNeutralZone;

        let warmup_pucks = self.config.warmup_pucks;

        let mut game = HQMGame::new(
            warmup_pucks,
            self.config.physics_config.clone(),
            self.config.blue_line_location,
        );
        game.history_length = if self.config.goal_replay { 850 } else { 0 };
        let puck_line_start = game.world.rink.width / 2.0 - 0.4 * ((warmup_pucks - 1) as f32);

        for i in 0..warmup_pucks {
            let pos = Point3::new(
                puck_line_start + 0.8 * (i as f32),
                1.5,
                game.world.rink.length / 2.0,
            );
            let rot = Rotation3::identity();
            game.world.create_puck_object(pos, rot);
        }
        game.time = self.config.time_warmup * 100;
        game
    }

    fn before_player_exit(&mut self, _server: &mut HQMServer, player_index: usize) {
        if let Some(x) = self
            .started_as_goalie
            .iter()
            .position(|x| *x == player_index)
        {
            self.started_as_goalie.remove(x);
        }
        self.preferred_positions.remove(&player_index);
        self.team_switch_timer.remove(&player_index);
    }

    fn after_player_force_off(&mut self, _server: &mut HQMServer, player_index: usize) {
        self.team_switch_timer.insert(player_index, 500);
    }

    fn get_number_of_players(&self) -> u32 {
        self.config.team_max as u32
    }
}

fn setup_position(
    positions: &mut HashMap<usize, (HQMTeam, String)>,
    players: &[(usize, Option<&str>)],
    allowed_positions: &[String],
    team: HQMTeam,
) {
    let mut available_positions = Vec::from(allowed_positions);

    // First, we try to give each player its preferred position
    for (player_index, player_position) in players.iter() {
        if let Some(player_position) = player_position {
            if let Some(x) = available_positions
                .iter()
                .position(|x| x == *player_position)
            {
                let s = available_positions.remove(x);
                positions.insert(*player_index, (team, s));
            }
        }
    }

    // Some players did not get their preferred positions because they didn't have one,
    // or because it was already taken
    for (player_index, player_position) in players.iter() {
        if !positions.contains_key(player_index) {
            let s = if let Some(x) = available_positions.iter().position(|x| x == "C") {
                // Someone needs to be C
                let x = available_positions.remove(x);
                (team, x)
            } else if !available_positions.is_empty() {
                // Give out the remaining positions
                let x = available_positions.remove(0);
                (team, x)
            } else {
                // Oh no, we're out of legal starting positions
                if let Some(player_position) = player_position {
                    (team, (*player_position).to_owned())
                } else {
                    (team, "C".to_owned())
                }
            };
            positions.insert(*player_index, s);
        }
    }

    if let Some(x) = available_positions.iter().position(|x| x == "C") {
        let mut change_index = None;
        for (player_index, _) in players.iter() {
            if change_index.is_none() {
                change_index = Some(player_index);
            }

            if let Some((_, pos)) = positions.get(player_index) {
                if pos != "G" {
                    change_index = Some(player_index);
                    break;
                }
            }
        }

        if let Some(change_index) = change_index {
            let c = available_positions.remove(x);
            positions.insert(*change_index, (team, c));
        }
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
                    if let Some((_, dual_control_team)) = player.object {
                        if dual_control_team == team {
                            return Some((i, movement, stick));
                        }
                    }
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use crate::hqm_match::setup_position;
    use migo_hqm_server::hqm_game::HQMTeam;
    use std::collections::HashMap;

    #[test]
    fn test1() {
        let allowed_positions: Vec<String> = vec![
            "C", "LW", "RW", "LD", "RD", "G", "LM", "RM", "LLM", "RRM", "LLD", "RRD", "CM", "CD",
            "LW2", "RW2", "LLW", "RRW",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        let c = "C";
        let lw = "LW";
        let rw = "RW";
        let g = "G";
        let mut res1 = HashMap::new();
        let players = vec![(0usize, None)];
        setup_position(
            &mut res1,
            players.as_ref(),
            &allowed_positions,
            HQMTeam::Red,
        );
        assert_eq!(res1[&0].1, "C");

        let mut res1 = HashMap::new();
        let players = vec![(0usize, Some(c))];
        setup_position(
            &mut res1,
            players.as_ref(),
            &allowed_positions,
            HQMTeam::Red,
        );
        assert_eq!(res1[&0].1, "C");

        let mut res1 = HashMap::new();
        let players = vec![(0usize, Some(lw))];
        setup_position(
            &mut res1,
            players.as_ref(),
            &allowed_positions,
            HQMTeam::Red,
        );
        assert_eq!(res1[&0].1, "C");

        let mut res1 = HashMap::new();
        let players = vec![(0usize, Some(g))];
        setup_position(
            &mut res1,
            players.as_ref(),
            &allowed_positions,
            HQMTeam::Red,
        );
        assert_eq!(res1[&0].1, "C");

        let mut res1 = HashMap::new();
        let players = vec![(0usize, Some(c)), (1usize, Some(lw))];
        setup_position(
            &mut res1,
            players.as_ref(),
            &allowed_positions,
            HQMTeam::Red,
        );
        assert_eq!(res1[&0].1, "C");
        assert_eq!(res1[&1].1, "LW");

        let mut res1 = HashMap::new();
        let players = vec![(0usize, None), (1usize, Some(lw))];
        setup_position(
            &mut res1,
            players.as_ref(),
            &allowed_positions,
            HQMTeam::Red,
        );
        assert_eq!(res1[&0].1, "C");
        assert_eq!(res1[&1].1, "LW");

        let mut res1 = HashMap::new();
        let players = vec![(0usize, Some(rw)), (1usize, Some(lw))];
        setup_position(
            &mut res1,
            players.as_ref(),
            &allowed_positions,
            HQMTeam::Red,
        );
        assert_eq!(res1[&0].1, "C");
        assert_eq!(res1[&1].1, "LW");

        let mut res1 = HashMap::new();
        let players = vec![(0usize, Some(g)), (1usize, Some(lw))];
        setup_position(
            &mut res1,
            players.as_ref(),
            &allowed_positions,
            HQMTeam::Red,
        );
        assert_eq!(res1[&0].1, "G");
        assert_eq!(res1[&1].1, "C");

        let mut res1 = HashMap::new();
        let players = vec![(0usize, Some(c)), (1usize, Some(c))];
        setup_position(
            &mut res1,
            players.as_ref(),
            &allowed_positions,
            HQMTeam::Red,
        );
        assert_eq!(res1[&0].1, "C");
        assert_eq!(res1[&1].1, "LW");
    }
}

pub fn add_touch(
    puck: &HQMPuck,
    entry: Entry<usize, VecDeque<HQMPuckTouch>>,
    player_index: usize,
    team: HQMTeam,
    time: u32,
) {
    let puck_pos = puck.body.pos.clone();
    let puck_speed = puck.body.linear_velocity.norm();

    let touches = entry.or_insert_with(|| VecDeque::new());
    let most_recent_touch = touches.front_mut();

    match most_recent_touch {
        Some(most_recent_touch)
            if most_recent_touch.player_index == player_index && most_recent_touch.team == team =>
        {
            most_recent_touch.puck_pos = puck_pos;
            most_recent_touch.last_time = time;
            most_recent_touch.puck_speed = puck_speed;
        }
        _ => {
            touches.truncate(15);
            touches.push_front(HQMPuckTouch {
                player_index,
                team,
                puck_pos,
                puck_speed,
                first_time: time,
                last_time: time,
            });
        }
    }
}

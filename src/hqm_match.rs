use nalgebra::{Point3, Rotation3, Vector3};
use tracing::info;

use migo_hqm_server::hqm_behaviour_extra::{
    add_touch, find_empty_dual_control, get_faceoff_positions, has_players_in_offensive_zone,
    HQMDualControlSetting, HQMIcingConfiguration, HQMIcingStatus, HQMOffsideConfiguration,
    HQMOffsideLineConfiguration, HQMOffsideStatus, HQMPuckTouch,
};
use migo_hqm_server::hqm_game::{
    HQMGame, HQMObjectIndex, HQMPhysicsConfiguration, HQMRinkFaceoffSpot, HQMRinkSide,
    HQMRulesState, HQMSkater, HQMTeam,
};
use migo_hqm_server::hqm_server::{
    HQMServer, HQMServerBehaviour, HQMServerPlayerIndex, HQMSpawnPoint,
};
use migo_hqm_server::hqm_simulate::HQMSimulationEvent;
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
    pub offside_line: HQMOffsideLineConfiguration,
    pub warmup_pucks: usize,
    pub physics_config: HQMPhysicsConfiguration,
    pub blue_line_location: f32,
    pub cheats_enabled: bool,
    pub use_mph: bool,
    pub dual_control: HQMDualControlSetting,
    pub goal_replay: bool,

    pub spawn_point: HQMSpawnPoint,
}

pub struct HQMMatchBehaviour {
    pub config: HQMMatchConfiguration,
    pub paused: bool,
    pause_timer: u32,
    is_pause_goal: bool,
    next_faceoff_spot: HQMRinkFaceoffSpot,
    icing_status: HQMIcingStatus,
    offside_status: HQMOffsideStatus,
    preferred_positions: HashMap<HQMServerPlayerIndex, String>,
    team_switch_timer: HashMap<HQMServerPlayerIndex, u32>,
    started_as_goalie: Vec<HQMServerPlayerIndex>,
    faceoff_game_step: u32,
    step_where_period_ended: u32,
    too_late_printed_this_period: bool,
    start_next_replay: Option<(u32, u32, Option<HQMServerPlayerIndex>)>,
    puck_touches: HashMap<HQMObjectIndex, VecDeque<HQMPuckTouch>>,
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
            offside_status: HQMOffsideStatus::Neutral,
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

    fn get_last_touch(&self, puck_index: HQMObjectIndex) -> Option<&HQMPuckTouch> {
        self.puck_touches.get(&puck_index).and_then(|x| x.front())
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
            server.spawn_skater(self, player_index, team, player_position, player_rotation);
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
            HQMOffsideStatus::Neutral
        };

        self.faceoff_game_step = server.game.game_step;
    }

    fn update_game_over(&mut self, server: &mut HQMServer) {
        let time_gameover = self.config.time_intermission * 100;
        let time_break = self.config.time_break * 100;

        let red_score = server.game.red_score;
        let blue_score = server.game.blue_score;
        let old_game_over = server.game.game_over;
        server.game.game_over =
            if server.game.period > self.config.periods && red_score != blue_score {
                true
            } else if self.config.mercy > 0
                && (red_score.saturating_sub(blue_score) >= self.config.mercy
                    || blue_score.saturating_sub(red_score) >= self.config.mercy)
            {
                true
            } else if self.config.first_to > 0
                && (red_score >= self.config.first_to || blue_score >= self.config.first_to)
            {
                true
            } else {
                false
            };
        if server.game.game_over && !old_game_over {
            self.pause_timer = self.pause_timer.max(time_gameover);
        } else if !server.game.game_over && old_game_over {
            self.pause_timer = self.pause_timer.max(time_break);
        }
    }

    fn call_goal(&mut self, server: &mut HQMServer, team: HQMTeam, puck_index: HQMObjectIndex) {
        let time_break = self.config.time_break * 100;

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

        server
            .messages
            .add_goal_message(team, goal_scorer_index, assist_index);

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

        server.messages.add_server_chat_message(s);

        if server.game.time < 1000 {
            let time = server.game.time;
            let seconds = time / 100;
            let centi = time % 100;

            let s = format!("{}.{:02} seconds left", seconds, centi);
            server.messages.add_server_chat_message(s);
        }

        self.pause_timer = time_break;
        self.is_pause_goal = true;

        self.update_game_over(server);

        let gamestep = server.game.game_step;

        if self.config.goal_replay {
            let force_view = goal_scorer_index.or(last_touch);
            self.start_next_replay = Some((
                self.faceoff_game_step.max(gamestep - 600),
                gamestep + 200,
                force_view,
            ));

            self.pause_timer = self.pause_timer.saturating_sub(800).max(400);
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

                    server.messages.add_server_chat_message(s);
                }
            }
        }
    }

    fn handle_puck_touch(
        &mut self,
        server: &mut HQMServer,
        player: HQMObjectIndex,
        puck_index: HQMObjectIndex,
    ) {
        if let Some((player_index, touching_team, _)) = server.players.get_from_object_index(player)
        {
            if let Some(puck) = server.game.world.objects.get_puck_mut(puck_index) {
                add_touch(
                    puck,
                    self.puck_touches.entry(puck_index),
                    player_index,
                    player,
                    touching_team,
                    server.game.time,
                );

                let other_team = touching_team.get_other_team();

                if let HQMOffsideStatus::Warning(team, p, i) = &self.offside_status {
                    if *team == touching_team {
                        let self_touch = player_index == *i;
                        let pass_origin = if player_index == *i {
                            puck.body.pos.clone()
                        } else {
                            p.clone()
                        };
                        self.call_offside(server, touching_team, &pass_origin, self_touch);
                        return;
                    }
                }
                if let HQMIcingStatus::Warning(team, p) = &self.icing_status {
                    if touching_team != *team {
                        if self.started_as_goalie.contains(&player_index) {
                            self.icing_status = HQMIcingStatus::No;
                            server
                                .messages
                                .add_server_chat_message_str("Icing waved off");
                        } else {
                            let copy = p.clone();
                            self.call_icing(server, other_team, &copy);
                        }
                    } else {
                        self.icing_status = HQMIcingStatus::No;
                        server
                            .messages
                            .add_server_chat_message_str("Icing waved off");
                    }
                } else if let HQMIcingStatus::NotTouched(_, _) = self.icing_status {
                    self.icing_status = HQMIcingStatus::No;
                }
            }
        }
    }

    fn handle_puck_entered_net(
        &mut self,
        server: &mut HQMServer,
        team: HQMTeam,
        puck: HQMObjectIndex,
    ) {
        match &self.offside_status {
            HQMOffsideStatus::Warning(offside_team, p, _) if *offside_team == team => {
                let copy = p.clone();
                self.call_offside(server, team, &copy, false);
            }
            HQMOffsideStatus::Offside(_) => {}
            _ => {
                self.call_goal(server, team, puck);
            }
        }
    }

    fn handle_puck_passed_goal_line(&mut self, server: &mut HQMServer, team: HQMTeam) {
        if let HQMIcingStatus::NotTouched(icing_team, p) = &self.icing_status {
            if team == *icing_team {
                match self.config.icing {
                    HQMIcingConfiguration::Touch => {
                        self.icing_status = HQMIcingStatus::Warning(team, p.clone());
                        server.messages.add_server_chat_message_str("Icing warning");
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

    fn puck_into_offside_zone(
        &mut self,
        server: &mut HQMServer,
        team: HQMTeam,
        puck_index: HQMObjectIndex,
    ) {
        if self.offside_status == HQMOffsideStatus::InOffensiveZone(team) {
            return;
        }
        if let Some(touch) = self.get_last_touch(puck_index) {
            if team == touch.team
                && has_players_in_offensive_zone(&server, team, Some(touch.player_index))
            {
                match self.config.offside {
                    HQMOffsideConfiguration::Delayed => {
                        self.offside_status = HQMOffsideStatus::Warning(
                            team,
                            touch.puck_pos.clone(),
                            touch.player_index,
                        );
                        server
                            .messages
                            .add_server_chat_message_str("Offside warning");
                    }
                    HQMOffsideConfiguration::Immediate => {
                        let puck_pos = touch.puck_pos.clone();

                        let self_touch = server
                            .game
                            .world
                            .objects
                            .get_skater(touch.skater_index)
                            .map_or(false, |skater: &HQMSkater| {
                                (&skater.body.pos - &puck_pos).norm() < 0.5
                            });

                        self.call_offside(server, team, &puck_pos, self_touch);
                    }
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

    fn handle_puck_entered_offensive_half(
        &mut self,
        server: &mut HQMServer,
        team: HQMTeam,
        puck: HQMObjectIndex,
    ) {
        if !matches!(&self.offside_status, HQMOffsideStatus::Offside(_))
            && self.config.offside_line == HQMOffsideLineConfiguration::Center
        {
            if let HQMOffsideStatus::Warning(warning_team, _, _) = self.offside_status {
                if warning_team != team {
                    server
                        .messages
                        .add_server_chat_message_str("Offside waved off");
                }
            }
            self.puck_into_offside_zone(server, team, puck);
        }
    }

    fn handle_puck_entered_offensive_zone(
        &mut self,
        server: &mut HQMServer,
        team: HQMTeam,
        puck: HQMObjectIndex,
    ) {
        if !matches!(&self.offside_status, HQMOffsideStatus::Offside(_))
            && self.config.offside_line == HQMOffsideLineConfiguration::OffensiveBlue
        {
            self.puck_into_offside_zone(server, team, puck);
        }
    }

    fn handle_puck_left_offensive_zone(&mut self, server: &mut HQMServer) {
        if !matches!(&self.offside_status, HQMOffsideStatus::Offside(_))
            && self.config.offside_line == HQMOffsideLineConfiguration::OffensiveBlue
        {
            if let HQMOffsideStatus::Warning(_, _, _) = self.offside_status {
                server
                    .messages
                    .add_server_chat_message_str("Offside waved off");
            }
            self.offside_status = HQMOffsideStatus::Neutral;
        }
    }

    fn handle_puck_reached_other_half(
        &mut self,
        _server: &mut HQMServer,
        team: HQMTeam,
        puck_index: HQMObjectIndex,
    ) {
        if let Some(touch) = self.get_last_touch(puck_index) {
            if team == touch.team && self.icing_status == HQMIcingStatus::No {
                self.icing_status = HQMIcingStatus::NotTouched(team, touch.puck_pos.clone());
            }
        }
    }

    fn handle_events(&mut self, server: &mut HQMServer, events: &[HQMSimulationEvent]) {
        for event in events {
            match event {
                HQMSimulationEvent::PuckEnteredNet { team, puck } => {
                    self.handle_puck_entered_net(server, *team, *puck);
                }
                HQMSimulationEvent::PuckTouch { player, puck, .. } => {
                    self.handle_puck_touch(server, *player, *puck);
                }
                HQMSimulationEvent::PuckReachedRedLine { team, puck } => {
                    self.handle_puck_reached_other_half(server, *team, *puck);
                }
                HQMSimulationEvent::PuckPassedGoalLine { team, puck: _ } => {
                    self.handle_puck_passed_goal_line(server, *team);
                }
                HQMSimulationEvent::PuckFullyEnteredOffensiveHalf { team, puck } => {
                    self.handle_puck_entered_offensive_half(server, *team, *puck);
                }
                HQMSimulationEvent::PuckEnteredOffensiveZone { team, puck } => {
                    self.handle_puck_entered_offensive_zone(server, *team, *puck);
                }
                HQMSimulationEvent::PuckLeftOffensiveZone { team: _, puck: _ } => {
                    self.handle_puck_left_offensive_zone(server);
                }
                _ => {}
            }
        }
    }

    fn call_offside(
        &mut self,
        server: &mut HQMServer,
        team: HQMTeam,
        pos: &Point3<f32>,
        self_touch: bool,
    ) {
        let time_break = self.config.time_break * 100;

        let side = if pos.x <= &server.game.world.rink.width / 2.0 {
            HQMRinkSide::Left
        } else {
            HQMRinkSide::Right
        };
        let lines_and_net = match team {
            HQMTeam::Red => &server.game.world.rink.red_lines_and_net,
            HQMTeam::Blue => &server.game.world.rink.blue_lines_and_net,
        };
        let faceoff_spot = if self_touch {
            match self.config.offside_line {
                HQMOffsideLineConfiguration::OffensiveBlue => {
                    HQMRinkFaceoffSpot::Offside(team.get_other_team(), side)
                }
                HQMOffsideLineConfiguration::Center => HQMRinkFaceoffSpot::Center,
            }
        } else if lines_and_net.offensive_line.point_past_middle_of_line(pos) {
            HQMRinkFaceoffSpot::Offside(team.get_other_team(), side)
        } else if lines_and_net.mid_line.point_past_middle_of_line(pos) {
            HQMRinkFaceoffSpot::Center
        } else if lines_and_net.defensive_line.point_past_middle_of_line(pos) {
            HQMRinkFaceoffSpot::Offside(team, side)
        } else {
            HQMRinkFaceoffSpot::DefensiveZone(team, side)
        };

        self.next_faceoff_spot = faceoff_spot;
        self.pause_timer = time_break;
        self.offside_status = HQMOffsideStatus::Offside(team);
        server.messages.add_server_chat_message_str("Offside");
    }

    fn call_icing(&mut self, server: &mut HQMServer, team: HQMTeam, pos: &Point3<f32>) {
        let time_break = self.config.time_break * 100;

        let side = if pos.x <= server.game.world.rink.width / 2.0 {
            HQMRinkSide::Left
        } else {
            HQMRinkSide::Right
        };

        self.next_faceoff_spot = HQMRinkFaceoffSpot::DefensiveZone(team, side);
        self.pause_timer = time_break;
        self.icing_status = HQMIcingStatus::Icing(team);
        server.messages.add_server_chat_message_str("Icing");
    }

    fn update_players(&mut self, server: &mut HQMServer) {
        let mut spectating_players = smallvec::SmallVec::<[_; 32]>::new();
        let mut joining_red = smallvec::SmallVec::<[_; 32]>::new();
        let mut joining_blue = smallvec::SmallVec::<[_; 32]>::new();
        for (player_index, player) in server.players.iter() {
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
                        let dual_control = self.config.dual_control == HQMDualControlSetting::Yes
                            || (self.config.dual_control == HQMDualControlSetting::Combined
                                && player.input.shift());
                        if player.input.join_red() {
                            joining_red.push((
                                player_index,
                                player.player_name.clone(),
                                dual_control,
                            ));
                        } else if player.input.join_blue() {
                            joining_blue.push((
                                player_index,
                                player.player_name.clone(),
                                dual_control,
                            ));
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
            server.remove_player_from_dual_control(self, player_index);
            server.move_to_spectator(self, player_index);
        }
        if !joining_red.is_empty() || !joining_blue.is_empty() {
            let (red_player_count, blue_player_count) = {
                let mut red_player_count = 0usize;
                let mut blue_player_count = 0usize;
                for (_, player) in server.players.iter() {
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

            fn add_player(
                behaviour: &mut HQMMatchBehaviour,
                player_index: HQMServerPlayerIndex,
                player_name: Rc<String>,
                server: &mut HQMServer,
                team: HQMTeam,
                spawn_point: HQMSpawnPoint,
                player_count: &mut usize,
                team_max: usize,
            ) {
                if *player_count >= team_max {
                    return;
                }

                if server
                    .spawn_skater_at_spawnpoint(behaviour, player_index, team, spawn_point)
                    .is_some()
                {
                    info!(
                        "{} ({}) has joined team {:?}",
                        player_name, player_index, team
                    );
                    *player_count += 1;

                    if let Some(x) = behaviour
                        .started_as_goalie
                        .iter()
                        .position(|x| *x == player_index)
                    {
                        behaviour.started_as_goalie.remove(x);
                    }
                }
            }
            fn add_player_dual_control(
                behaviour: &mut HQMMatchBehaviour,
                player_index: HQMServerPlayerIndex,
                player_name: Rc<String>,
                server: &mut HQMServer,
                team: HQMTeam,
                spawn_point: HQMSpawnPoint,
                player_count: &mut usize,
                team_max: usize,
            ) {
                let current_empty = find_empty_dual_control(server, team);

                match current_empty {
                    Some((index, movement @ Some(_), None)) => {
                        server.update_dual_control(behaviour, index, movement, Some(player_index));
                    }
                    Some((index, None, stick @ Some(_))) => {
                        server.update_dual_control(behaviour, index, Some(player_index), stick);
                    }
                    _ => {
                        if *player_count >= team_max {}

                        if let Some((dual_control_player_index, _)) = server
                            .spawn_dual_control_skater_at_spawnpoint(
                                behaviour,
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

                            if let Some(x) = behaviour
                                .started_as_goalie
                                .iter()
                                .position(|x| *x == dual_control_player_index)
                            {
                                behaviour.started_as_goalie.remove(x);
                            }
                        }
                    }
                }
            }

            for (player_index, player_name, dual_control) in joining_red {
                if dual_control {
                    add_player_dual_control(
                        self,
                        player_index,
                        player_name,
                        server,
                        HQMTeam::Red,
                        self.config.spawn_point,
                        &mut new_red_player_count,
                        self.config.team_max,
                    )
                } else {
                    add_player(
                        self,
                        player_index,
                        player_name,
                        server,
                        HQMTeam::Red,
                        self.config.spawn_point,
                        &mut new_red_player_count,
                        self.config.team_max,
                    )
                }
            }
            for (player_index, player_name, dual_control) in joining_blue {
                if dual_control {
                    add_player_dual_control(
                        self,
                        player_index,
                        player_name,
                        server,
                        HQMTeam::Blue,
                        self.config.spawn_point,
                        &mut new_blue_player_count,
                        self.config.team_max,
                    )
                } else {
                    add_player(
                        self,
                        player_index,
                        player_name,
                        server,
                        HQMTeam::Blue,
                        self.config.spawn_point,
                        &mut new_blue_player_count,
                        self.config.team_max,
                    )
                }
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

    fn cheat(&mut self, server: &mut HQMServer, player_index: HQMServerPlayerIndex, arg: &str) {
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

    fn set_team_size(
        &mut self,
        server: &mut HQMServer,
        player_index: HQMServerPlayerIndex,
        size: &str,
    ) {
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

                        server.messages.add_server_chat_message(msg);
                    }
                }
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    fn set_icing_rule(
        &mut self,
        server: &mut HQMServer,
        player_index: HQMServerPlayerIndex,
        rule: &str,
    ) {
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

                        server.messages.add_server_chat_message(msg);
                    }
                    "notouch" => {
                        self.config.icing = HQMIcingConfiguration::NoTouch;
                        info!(
                            "{} ({}) enabled no-touch icing",
                            player.player_name, player_index
                        );
                        let msg = format!("No-touch icing enabled by {}", player.player_name);

                        server.messages.add_server_chat_message(msg);
                    }
                    "off" => {
                        self.config.icing = HQMIcingConfiguration::Off;
                        info!("{} ({}) disabled icing", player.player_name, player_index);
                        let msg = format!("Icing disabled by {}", player.player_name);

                        server.messages.add_server_chat_message(msg);
                    }
                    _ => {}
                }
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    fn set_offside_line(
        &mut self,
        server: &mut HQMServer,
        player_index: HQMServerPlayerIndex,
        rule: &str,
    ) {
        if let Some(player) = server.players.get(player_index) {
            if player.is_admin {
                match rule {
                    "blue" => {
                        self.config.offside_line = HQMOffsideLineConfiguration::OffensiveBlue;
                        info!(
                            "{} ({}) set blue line as offside line",
                            player.player_name, player_index
                        );
                        let msg =
                            format!("Blue line set as offside line by {}", player.player_name);

                        server.messages.add_server_chat_message(msg);
                    }
                    "center" => {
                        self.config.offside_line = HQMOffsideLineConfiguration::Center;
                        info!(
                            "{} ({}) set center line as offside line",
                            player.player_name, player_index
                        );
                        let msg =
                            format!("Center line set as offside line by {}", player.player_name);

                        server.messages.add_server_chat_message(msg);
                    }
                    _ => {}
                }
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    fn set_offside_rule(
        &mut self,
        server: &mut HQMServer,
        player_index: HQMServerPlayerIndex,
        rule: &str,
    ) {
        if let Some(player) = server.players.get(player_index) {
            if player.is_admin {
                match rule {
                    "on" | "delayed" => {
                        self.config.offside = HQMOffsideConfiguration::Delayed;
                        info!("{} ({}) enabled offside", player.player_name, player_index);
                        let msg = format!("Offside enabled by {}", player.player_name);

                        server.messages.add_server_chat_message(msg);
                    }
                    "imm" | "immediate" => {
                        self.config.offside = HQMOffsideConfiguration::Immediate;
                        info!(
                            "{} ({}) enabled immediate offside",
                            player.player_name, player_index
                        );
                        let msg = format!("Immediate offside enabled by {}", player.player_name);

                        server.messages.add_server_chat_message(msg);
                    }
                    "off" => {
                        self.config.offside = HQMOffsideConfiguration::Off;
                        info!("{} ({}) disabled offside", player.player_name, player_index);
                        let msg = format!("Offside disabled by {}", player.player_name);

                        server.messages.add_server_chat_message(msg);
                    }
                    _ => {}
                }
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    fn set_goal_replay(
        &mut self,
        server: &mut HQMServer,
        player_index: HQMServerPlayerIndex,
        setting: &str,
    ) {
        if let Some(player) = server.players.get(player_index) {
            if player.is_admin {
                match setting {
                    "on" => {
                        self.config.goal_replay = true;
                        let msg = format!("Goal replays enabled by {}", player.player_name);
                        server.messages.add_server_chat_message(msg);
                    }
                    "off" => {
                        self.config.goal_replay = false;
                        let msg = format!("Goal replays disabled by {}", player.player_name);
                        server.messages.add_server_chat_message(msg);
                    }
                    _ => {}
                }
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    fn set_first_to_rule(
        &mut self,
        server: &mut HQMServer,
        player_index: HQMServerPlayerIndex,
        num: &str,
    ) {
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
                        server.messages.add_server_chat_message(msg);
                    } else {
                        info!(
                            "{} ({}) disabled first-to-goals rule",
                            player.player_name, player_index
                        );
                        let msg = format!("First-to-goals rule disabled by {}", player.player_name);
                        server.messages.add_server_chat_message(msg);
                    }
                }
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    fn set_mercy_rule(
        &mut self,
        server: &mut HQMServer,
        player_index: HQMServerPlayerIndex,
        num: &str,
    ) {
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
                        server.messages.add_server_chat_message(msg);
                    } else {
                        info!(
                            "{} ({}) disabled mercy rule",
                            player.player_name, player_index
                        );
                        let msg = format!("Mercy rule disabled by {}", player.player_name);
                        server.messages.add_server_chat_message(msg);
                    }
                }
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    fn faceoff(&mut self, server: &mut HQMServer, player_index: HQMServerPlayerIndex) {
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
                    server.messages.add_server_chat_message(msg);
                } else {
                    server.admin_deny_message(player_index);
                }
            }
        }
    }

    fn reset_game(&mut self, server: &mut HQMServer, player_index: HQMServerPlayerIndex) {
        if let Some(player) = server.players.get(player_index) {
            if player.is_admin {
                info!("{} ({}) reset game", player.player_name, player_index);
                let msg = format!("Game reset by {}", player.player_name);

                server.new_game(self.create_game());

                server.messages.add_server_chat_message(msg);
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    fn start_game(&mut self, server: &mut HQMServer, player_index: HQMServerPlayerIndex) {
        if let Some(player) = server.players.get(player_index) {
            if player.is_admin {
                if server.game.period == 0 && server.game.time > 1 {
                    info!("{} ({}) started game", player.player_name, player_index);
                    let msg = format!("Game started by {}", player.player_name);
                    self.paused = false;
                    server.game.time = 1;

                    server.messages.add_server_chat_message(msg);
                }
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    fn pause(&mut self, server: &mut HQMServer, player_index: HQMServerPlayerIndex) {
        if let Some(player) = server.players.get(player_index) {
            if player.is_admin {
                self.paused = true;
                if self.pause_timer > 0 && self.pause_timer < self.config.time_break {
                    // If we're currently in a break, with very little time left,
                    // we reset the timer
                    self.pause_timer = self.config.time_break;
                }
                info!("{} ({}) paused game", player.player_name, player_index);
                let msg = format!("Game paused by {}", player.player_name);
                server.messages.add_server_chat_message(msg);
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    fn unpause(&mut self, server: &mut HQMServer, player_index: HQMServerPlayerIndex) {
        if let Some(player) = server.players.get(player_index) {
            if player.is_admin {
                self.paused = false;
                info!("{} ({}) resumed game", player.player_name, player_index);
                let msg = format!("Game resumed by {}", player.player_name);

                server.messages.add_server_chat_message(msg);
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    fn set_clock(
        &mut self,
        server: &mut HQMServer,
        input_minutes: u32,
        input_seconds: u32,
        player_index: HQMServerPlayerIndex,
    ) {
        if let Some(player) = server.players.get(player_index) {
            if player.is_admin {
                server.game.time = (input_minutes * 60 * 100) + (input_seconds * 100);

                info!(
                    "Clock set to {}:{} by {} ({})",
                    input_minutes, input_seconds, player.player_name, player_index
                );
                let msg = format!("Clock set by {}", player.player_name);
                server.messages.add_server_chat_message(msg);
                self.update_game_over(server);
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    fn set_score(
        &mut self,
        server: &mut HQMServer,
        input_team: HQMTeam,
        input_score: u32,
        player_index: HQMServerPlayerIndex,
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
                        server.messages.add_server_chat_message(msg);
                    }
                    HQMTeam::Blue => {
                        server.game.blue_score = input_score;

                        info!(
                            "{} ({}) changed blue score to {}",
                            player.player_name, player_index, input_score
                        );
                        let msg = format!("Blue score changed by {}", player.player_name);
                        server.messages.add_server_chat_message(msg);
                    }
                }
                self.update_game_over(server);
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    fn set_period(
        &mut self,
        server: &mut HQMServer,
        input_period: u32,
        player_index: HQMServerPlayerIndex,
    ) {
        if let Some(player) = server.players.get(player_index) {
            if player.is_admin {
                server.game.period = input_period;

                info!(
                    "{} ({}) set period to {}",
                    player.player_name, player_index, input_period
                );
                let msg = format!("Period set by {}", player.player_name);
                server.messages.add_server_chat_message(msg);
                self.update_game_over(server);
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    fn set_period_num(
        &mut self,
        server: &mut HQMServer,
        input_period: u32,
        player_index: HQMServerPlayerIndex,
    ) {
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
                server.messages.add_server_chat_message(msg);
                self.update_game_over(server);
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    fn set_preferred_faceoff_position(
        &mut self,
        server: &mut HQMServer,
        player_index: HQMServerPlayerIndex,
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
                server.messages.add_server_chat_message(msg);
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
                    self.update_game_over(server);
                }
            }
        }
        server.game.goal_message_timer = if self.is_pause_goal {
            self.pause_timer
        } else {
            0
        };
    }

    fn force_player_off_ice(
        &mut self,
        server: &mut HQMServer,
        admin_player_index: HQMServerPlayerIndex,
        force_player_index: HQMServerPlayerIndex,
    ) {
        if let Some(player) = server.players.get(admin_player_index) {
            if player.is_admin {
                let admin_player_name = player.player_name.clone();

                if let Some(force_player) = server.players.get(force_player_index) {
                    let force_player_name = force_player.player_name.clone();
                    if server.move_to_spectator(self, force_player_index) {
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
                        server.messages.add_server_chat_message(msg);
                        self.team_switch_timer.insert(force_player_index, 500);
                    }
                }
            } else {
                server.admin_deny_message(admin_player_index);
                return;
            }
        }
    }
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

            if let HQMOffsideStatus::Warning(team, _, _) = self.offside_status {
                if !has_players_in_offensive_zone(server, team, None) {
                    self.offside_status = HQMOffsideStatus::InOffensiveZone(team);
                    server
                        .messages
                        .add_server_chat_message_str("Offside waved off");
                }
            }

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
                server.messages.add_server_chat_message_str("Goal replay");
                self.start_next_replay = None;
            }
        }
    }

    fn handle_command(
        &mut self,
        server: &mut HQMServer,
        command: &str,
        arg: &str,
        player_index: HQMServerPlayerIndex,
    ) {
        match command {
            "set" => {
                let args = arg.split(" ").collect::<Vec<&str>>();
                if args.len() > 1 {
                    match args[0] {
                        "redscore" => {
                            if let Ok(input_score) = args[1].parse::<u32>() {
                                self.set_score(server, HQMTeam::Red, input_score, player_index);
                            }
                        }
                        "bluescore" => {
                            if let Ok(input_score) = args[1].parse::<u32>() {
                                self.set_score(server, HQMTeam::Blue, input_score, player_index);
                            }
                        }
                        "period" => {
                            if let Ok(input_period) = args[1].parse::<u32>() {
                                self.set_period(server, input_period, player_index);
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
                                    self.set_clock(
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
                        "offsideline" => {
                            if let Some(arg) = args.get(1) {
                                self.set_offside_line(server, player_index, arg);
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
            "fs" => {
                if let Ok(force_player_index) = arg.parse::<HQMServerPlayerIndex>() {
                    self.force_player_off_ice(server, player_index, force_player_index);
                }
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
                let offside_line_str = if self.config.offside != HQMOffsideConfiguration::Off
                    && self.config.offside_line == HQMOffsideLineConfiguration::Center
                {
                    " (center line)"
                } else {
                    ""
                };
                let icing_str = match self.config.icing {
                    HQMIcingConfiguration::Off => "Icing disabled",
                    HQMIcingConfiguration::Touch => "Icing enabled",
                    HQMIcingConfiguration::NoTouch => "No-touch icing enabled",
                };
                let msg = format!("{}{}, {}", offside_str, offside_line_str, icing_str);
                server
                    .messages
                    .add_directed_server_chat_message(msg, player_index);
                if self.config.mercy > 0 {
                    let msg = format!("Mercy rule when team leads by {} goals", self.config.mercy);
                    server
                        .messages
                        .add_directed_server_chat_message(msg, player_index);
                }
                if self.config.first_to > 0 {
                    let msg = format!("Game ends when team scores {} goals", self.config.first_to);
                    server
                        .messages
                        .add_directed_server_chat_message(msg, player_index);
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
        self.pause_timer = 0;
        self.next_faceoff_spot = HQMRinkFaceoffSpot::Center;
        self.icing_status = HQMIcingStatus::No;
        self.offside_status = HQMOffsideStatus::Neutral;

        let warmup_pucks = self.config.warmup_pucks;

        let mut game = HQMGame::new(
            warmup_pucks,
            self.config.physics_config.clone(),
            self.config.blue_line_location,
        );
        game.history_length = 1000;
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

    fn before_player_exit(&mut self, _server: &mut HQMServer, player_index: HQMServerPlayerIndex) {
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

    fn get_number_of_players(&self) -> u32 {
        self.config.team_max as u32
    }
}

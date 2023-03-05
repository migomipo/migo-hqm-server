use nalgebra::{Point3, Rotation3, Vector3};
use tracing::info;

use migo_hqm_server::hqm_behaviour_extra::{
    add_touch, find_empty_dual_control, get_faceoff_positions, has_players_in_offensive_zone,
    is_past_line, HQMDualControlSetting, HQMIcingConfiguration, HQMIcingStatus,
    HQMOffsideConfiguration, HQMOffsideLineConfiguration, HQMOffsideStatus, HQMPass,
    HQMPassPosition, HQMPuckTouch, HQMTwoLinePassConfiguration, HQMTwoLinePassStatus,
};
use migo_hqm_server::hqm_game::{
    HQMGame, HQMObjectIndex, HQMPhysicsConfiguration, HQMRinkFaceoffSpot, HQMRinkSide,
    HQMRulesState, HQMTeam,
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
    pub twoline_pass: HQMTwoLinePassConfiguration,
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
    pub(crate) pause_timer: u32,
    is_pause_goal: bool,
    next_faceoff_spot: HQMRinkFaceoffSpot,
    icing_status: HQMIcingStatus,
    offside_status: HQMOffsideStatus,
    twoline_pass_status: HQMTwoLinePassStatus,
    pass: Option<HQMPass>,
    pub(crate) preferred_positions: HashMap<HQMServerPlayerIndex, String>,
    pub(crate) team_switch_timer: HashMap<HQMServerPlayerIndex, u32>,
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
            twoline_pass_status: HQMTwoLinePassStatus::No,
            pass: None,
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
        self.twoline_pass_status = HQMTwoLinePassStatus::No;
        self.pass = None;

        self.faceoff_game_step = server.game.game_step;
    }

    pub(crate) fn update_game_over(&mut self, server: &mut HQMServer) {
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
                let side = if puck.body.pos.x <= &server.game.world.rink.width / 2.0 {
                    HQMRinkSide::Left
                } else {
                    HQMRinkSide::Right
                };
                self.pass = Some(HQMPass {
                    team: touching_team,
                    side,
                    from: None,
                    player: player_index,
                });

                let other_team = touching_team.get_other_team();

                if let HQMOffsideStatus::Warning(team, side, position, i) = self.offside_status {
                    if team == touching_team {
                        let self_touch = player_index == i;

                        self.call_offside(server, touching_team, side, position, self_touch);
                        return;
                    }
                }
                if let HQMTwoLinePassStatus::Warning(team, side, position, ref i) =
                    self.twoline_pass_status
                {
                    if team == touching_team && i.contains(&player_index) {
                        self.call_twoline_pass(server, touching_team, side, position);
                        return;
                    } else {
                        self.twoline_pass_status = HQMTwoLinePassStatus::No;
                        server
                            .messages
                            .add_server_chat_message_str("Two-line pass waved off");
                    }
                }
                if let HQMIcingStatus::Warning(team, side) = self.icing_status {
                    if touching_team != team {
                        if self.started_as_goalie.contains(&player_index) {
                            self.icing_status = HQMIcingStatus::No;
                            server
                                .messages
                                .add_server_chat_message_str("Icing waved off");
                        } else {
                            self.call_icing(server, other_team, side);
                        }
                    } else {
                        self.icing_status = HQMIcingStatus::No;
                        server
                            .messages
                            .add_server_chat_message_str("Icing waved off");
                    }
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
        match self.offside_status {
            HQMOffsideStatus::Warning(offside_team, side, position, _) if offside_team == team => {
                self.call_offside(server, team, side, position, false);
            }
            HQMOffsideStatus::Offside(_) => {}
            _ => {
                self.call_goal(server, team, puck);
            }
        }
    }

    fn handle_puck_passed_goal_line(&mut self, server: &mut HQMServer, team: HQMTeam) {
        if let Some(HQMPass {
            team: icing_team,
            side,
            from: Some(transition),
            ..
        }) = self.pass
        {
            if team == icing_team && transition <= HQMPassPosition::ReachedCenter {
                match self.config.icing {
                    HQMIcingConfiguration::Touch => {
                        self.icing_status = HQMIcingStatus::Warning(team, side);
                        server.messages.add_server_chat_message_str("Icing warning");
                    }
                    HQMIcingConfiguration::NoTouch => {
                        self.call_icing(server, team, side);
                    }
                    HQMIcingConfiguration::Off => {}
                }
            }
        }
    }

    fn puck_into_offside_zone(&mut self, server: &mut HQMServer, team: HQMTeam) {
        if self.offside_status == HQMOffsideStatus::InOffensiveZone(team) {
            return;
        }
        if let Some(HQMPass {
            team: pass_team,
            side,
            from: transition,
            player,
        }) = self.pass
        {
            if team == pass_team && has_players_in_offensive_zone(&server, team, Some(player)) {
                match self.config.offside {
                    HQMOffsideConfiguration::Delayed => {
                        self.offside_status =
                            HQMOffsideStatus::Warning(team, side, transition, player);
                        server
                            .messages
                            .add_server_chat_message_str("Offside warning");
                    }
                    HQMOffsideConfiguration::Immediate => {
                        self.call_offside(server, team, side, transition, false);
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

    fn handle_puck_entered_offensive_half(&mut self, server: &mut HQMServer, team: HQMTeam) {
        if !matches!(&self.offside_status, HQMOffsideStatus::Offside(_))
            && self.config.offside_line == HQMOffsideLineConfiguration::Center
        {
            self.puck_into_offside_zone(server, team);
        }
        if let HQMOffsideStatus::Warning(warning_team, _, _, _) = self.offside_status {
            if warning_team != team {
                server
                    .messages
                    .add_server_chat_message_str("Offside waved off");
            }
        }
        if let Some(HQMPass {
            team: pass_team,
            side,
            from: Some(from),
            player: pass_player,
        }) = self.pass
        {
            if self.twoline_pass_status == HQMTwoLinePassStatus::No && pass_team == team {
                let is_regular_twoline_pass_active = self.config.twoline_pass
                    == HQMTwoLinePassConfiguration::Double
                    || self.config.twoline_pass == HQMTwoLinePassConfiguration::On;
                if from <= HQMPassPosition::ReachedOwnBlue && is_regular_twoline_pass_active {
                    self.check_twoline_pass(server, team, side, from, pass_player, false);
                }
            }
        }
    }

    fn handle_puck_entered_offensive_zone(&mut self, server: &mut HQMServer, team: HQMTeam) {
        if !matches!(&self.offside_status, HQMOffsideStatus::Offside(_))
            && self.config.offside_line == HQMOffsideLineConfiguration::OffensiveBlue
        {
            self.puck_into_offside_zone(server, team);
        }
        if let Some(HQMPass {
            team: pass_team,
            side,
            from: Some(from),
            player: pass_player,
        }) = self.pass
        {
            if self.twoline_pass_status == HQMTwoLinePassStatus::No && pass_team == team {
                let is_forward_twoline_pass_active = self.config.twoline_pass
                    == HQMTwoLinePassConfiguration::Double
                    || self.config.twoline_pass == HQMTwoLinePassConfiguration::Forward;
                let is_threeline_pass_active =
                    self.config.twoline_pass == HQMTwoLinePassConfiguration::ThreeLine;
                if (from <= HQMPassPosition::ReachedCenter && is_forward_twoline_pass_active)
                    || from <= HQMPassPosition::ReachedOwnBlue && is_threeline_pass_active
                {
                    self.check_twoline_pass(server, team, side, from, pass_player, true);
                }
            }
        }
    }

    fn check_twoline_pass(
        &mut self,
        server: &mut HQMServer,
        team: HQMTeam,
        side: HQMRinkSide,
        from: HQMPassPosition,
        pass_player: HQMServerPlayerIndex,
        is_offensive_line: bool,
    ) {
        let team_line = match team {
            HQMTeam::Red => &server.game.world.rink.red_lines_and_net,
            HQMTeam::Blue => &server.game.world.rink.blue_lines_and_net,
        };
        let line = if is_offensive_line {
            &team_line.offensive_line
        } else {
            &team_line.mid_line
        };
        let mut players_past_line = vec![];
        for (player_index, player) in server.players.iter() {
            if player_index == pass_player {
                continue;
            }
            if let Some(player) = player {
                if is_past_line(server, player, team, line) {
                    players_past_line.push(player_index);
                }
            }
        }
        if !players_past_line.is_empty() {
            self.twoline_pass_status =
                HQMTwoLinePassStatus::Warning(team, side, from, players_past_line);
            server
                .messages
                .add_server_chat_message_str("Two-line pass warning");
        }
    }

    fn handle_puck_passed_defensive_line(&mut self, server: &mut HQMServer, team: HQMTeam) {
        if !matches!(&self.offside_status, HQMOffsideStatus::Offside(_))
            && self.config.offside_line == HQMOffsideLineConfiguration::OffensiveBlue
        {
            if let HQMOffsideStatus::Warning(t, _, _, _) = self.offside_status {
                if team.get_other_team() == t {
                    server
                        .messages
                        .add_server_chat_message_str("Offside waved off");
                }
            }
            self.offside_status = HQMOffsideStatus::Neutral;
        }
    }

    fn update_pass(&mut self, team: HQMTeam, p: HQMPassPosition) {
        if let Some(pass) = &mut self.pass {
            if pass.team == team && pass.from.is_none() {
                pass.from = Some(p);
            }
        }
    }

    fn check_wave_off_twoline(&mut self, server: &mut HQMServer, team: HQMTeam) {
        if let HQMTwoLinePassStatus::Warning(warning_team, _, _, _) = self.twoline_pass_status {
            if team != warning_team {
                self.twoline_pass_status = HQMTwoLinePassStatus::No;
                server
                    .messages
                    .add_server_chat_message_str("Two-line pass waved off");
            }
        }
    }

    fn handle_events(&mut self, server: &mut HQMServer, events: &[HQMSimulationEvent]) {
        for event in events {
            match *event {
                HQMSimulationEvent::PuckEnteredNet { team, puck } => {
                    self.handle_puck_entered_net(server, team, puck);
                }
                HQMSimulationEvent::PuckTouch { player, puck, .. } => {
                    self.handle_puck_touch(server, player, puck);
                }
                HQMSimulationEvent::PuckReachedDefensiveLine { team, puck: _ } => {
                    self.check_wave_off_twoline(server, team);
                    self.update_pass(team, HQMPassPosition::ReachedOwnBlue);
                }
                HQMSimulationEvent::PuckPassedDefensiveLine { team, puck: _ } => {
                    self.update_pass(team, HQMPassPosition::PassedOwnBlue);
                    self.handle_puck_passed_defensive_line(server, team);
                }
                HQMSimulationEvent::PuckReachedCenterLine { team, puck: _ } => {
                    self.check_wave_off_twoline(server, team);
                    self.update_pass(team, HQMPassPosition::ReachedCenter);
                }
                HQMSimulationEvent::PuckPassedCenterLine { team, puck: _ } => {
                    self.update_pass(team, HQMPassPosition::PassedCenter);
                    self.handle_puck_entered_offensive_half(server, team);
                }
                HQMSimulationEvent::PuckReachedOffensiveZone { team, puck: _ } => {
                    self.update_pass(team, HQMPassPosition::ReachedOffensive);
                }
                HQMSimulationEvent::PuckEnteredOffensiveZone { team, puck: _ } => {
                    self.update_pass(team, HQMPassPosition::PassedOffensive);
                    self.handle_puck_entered_offensive_zone(server, team);
                }
                HQMSimulationEvent::PuckPassedGoalLine { team, puck: _ } => {
                    self.handle_puck_passed_goal_line(server, team);
                }
                _ => {}
            }
        }
    }

    fn call_offside(
        &mut self,
        server: &mut HQMServer,
        team: HQMTeam,
        side: HQMRinkSide,
        position: Option<HQMPassPosition>,
        self_touch: bool,
    ) {
        let time_break = self.config.time_break * 100;

        let faceoff_spot = if self_touch {
            match self.config.offside_line {
                HQMOffsideLineConfiguration::OffensiveBlue => {
                    HQMRinkFaceoffSpot::Offside(team.get_other_team(), side)
                }
                HQMOffsideLineConfiguration::Center => HQMRinkFaceoffSpot::Center,
            }
        } else {
            match position {
                Some(p) if p <= HQMPassPosition::ReachedOwnBlue => {
                    HQMRinkFaceoffSpot::DefensiveZone(team, side)
                }
                Some(p) if p <= HQMPassPosition::ReachedCenter => {
                    HQMRinkFaceoffSpot::Offside(team, side)
                }
                _ => HQMRinkFaceoffSpot::Center,
            }
        };

        self.next_faceoff_spot = faceoff_spot;
        self.pause_timer = time_break;
        self.offside_status = HQMOffsideStatus::Offside(team);
        server.messages.add_server_chat_message_str("Offside");
    }

    fn call_twoline_pass(
        &mut self,
        server: &mut HQMServer,
        team: HQMTeam,
        side: HQMRinkSide,
        position: HQMPassPosition,
    ) {
        let time_break = self.config.time_break * 100;

        let faceoff_spot = if position <= HQMPassPosition::ReachedOwnBlue {
            HQMRinkFaceoffSpot::DefensiveZone(team, side)
        } else if position <= HQMPassPosition::ReachedCenter {
            HQMRinkFaceoffSpot::Offside(team, side)
        } else {
            HQMRinkFaceoffSpot::Center
        };

        self.next_faceoff_spot = faceoff_spot;
        self.pause_timer = time_break;
        self.twoline_pass_status = HQMTwoLinePassStatus::Offside(team);
        server.messages.add_server_chat_message_str("Two-line pass");
    }

    fn call_icing(&mut self, server: &mut HQMServer, team: HQMTeam, side: HQMRinkSide) {
        let time_break = self.config.time_break * 100;

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

            if let HQMOffsideStatus::Warning(team, _, _, _) = self.offside_status {
                if !has_players_in_offensive_zone(server, team, None) {
                    self.offside_status = HQMOffsideStatus::InOffensiveZone(team);
                    server
                        .messages
                        .add_server_chat_message_str("Offside waved off");
                }
            }

            let rules_state = if matches!(self.offside_status, HQMOffsideStatus::Offside(_))
                || matches!(self.twoline_pass_status, HQMTwoLinePassStatus::Offside(_))
            {
                HQMRulesState::Offside
            } else if matches!(self.icing_status, HQMIcingStatus::Icing(_)) {
                HQMRulesState::Icing
            } else {
                let icing_warning = matches!(self.icing_status, HQMIcingStatus::Warning(_, _));
                let offside_warning =
                    matches!(self.offside_status, HQMOffsideStatus::Warning(_, _, _, _))
                        || matches!(
                            self.twoline_pass_status,
                            HQMTwoLinePassStatus::Warning(_, _, _, _)
                        );
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
                        "twolinepass" => {
                            if let Some(arg) = args.get(1) {
                                self.set_twoline_pass(server, player_index, arg);
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
                self.msg_rules(server, player_index);
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
        self.twoline_pass_status = HQMTwoLinePassStatus::No;

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

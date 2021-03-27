use crate::hqm_game::{HQMTeam, HQMGameWorld, HQMGameObject, HQMOffsideStatus, HQMIcingStatus, HQMMessage};
use crate::hqm_server::{HQMServer, HQMOffsideConfiguration, HQMIcingConfiguration, HQMConnectedPlayer};
use crate::hqm_simulate::HQMSimulationEvent;
use nalgebra::{Vector3, Point3, Matrix3};
use std::collections::HashMap;

impl HQMServer {
    fn call_goal (& mut self, team: HQMTeam, puck: usize) {
        let (new_score, opponent_score) = match team {
            HQMTeam::Red => {
                self.game.red_score += 1;
                (self.game.red_score, self.game.blue_score)
            }
            HQMTeam::Blue => {
                self.game.blue_score += 1;
                (self.game.blue_score, self.game.red_score)
            }
        };

        self.game.time_break = self.config.time_break *100;
        self.game.is_intermission_goal = true;
        self.game.next_faceoff_spot = self.game.world.rink.center_faceoff_spot.clone();

        let game_over = if self.game.period > 3 && self.game.red_score != self.game.blue_score {
            true
        } else if self.config.mercy > 0 && (new_score - opponent_score) >= self.config.mercy {
            true
        } else if self.config.first_to > 0 && new_score >= self.config.first_to {
            true
        } else {
            false
        };

        if game_over {
            self.game.time_break = self.config.time_intermission*100;
            self.game.game_over = true;
        }

        let mut goal_scorer_index = None;
        let mut assist_index = None;

        if let HQMGameObject::Puck(this_puck) = & mut self.game.world.objects[puck] {
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

        let message = HQMMessage::Goal {
            team,
            goal_player_index: goal_scorer_index,
            assist_player_index: assist_index
        };
        self.add_global_message(message, true);
    }

    fn call_offside(&mut self, team: HQMTeam, pass_origin: &Point3<f32>) {

        self.game.next_faceoff_spot = self.game.world.rink.get_offside_faceoff_spot(pass_origin, team);
        self.game.time_break = self.config.time_break *100;
        self.game.offside_status = HQMOffsideStatus::Offside(team);
        self.add_server_chat_message(String::from("Offside"));
    }

    fn call_icing(& mut self, team: HQMTeam, pass_origin: &Point3<f32>) {
        self.game.next_faceoff_spot = self.game.world.rink.get_icing_faceoff_spot(pass_origin, team);
        self.game.time_break = self.config.time_break *100;
        self.game.icing_status = HQMIcingStatus::Icing(team);
        self.add_server_chat_message(String::from("Icing"));
    }

    pub(crate) fn handle_events (& mut self, events: Vec<HQMSimulationEvent>) {
        if self.game.offside_status.is_offside()
            || self.game.icing_status.is_icing()
            || self.game.period == 0
            || self.game.time == 0
            || self.game.time_break > 0
            || self.game.paused {
            return;
        }
        for event in events {
            match event {
                HQMSimulationEvent::PuckEnteredNet {
                    team, puck
                } => {
                    match &self.game.offside_status {
                        HQMOffsideStatus::Warning(offside_team, p, _) if *offside_team == team => {
                            let copy = p.clone();
                            self.call_offside(team, &copy);
                        }
                        HQMOffsideStatus::Offside(_) => {},
                        _ => {
                            self.call_goal(team, puck);
                        }
                    }
                },
                HQMSimulationEvent::PuckTouch {
                    player, puck
                } => {
                    // Get connected player index from skater
                    if let HQMGameObject::Player(skater) = & self.game.world.objects[player] {
                        let this_connected_player_index = skater.connected_player_index;
                        let touching_team = skater.team;
                        let faceoff_position = skater.faceoff_position.clone();

                        if let HQMGameObject::Puck(puck) = & mut self.game.world.objects[puck] {
                            puck.add_touch(this_connected_player_index, touching_team, self.game.time);

                            let other_team = match touching_team {
                                HQMTeam::Red => HQMTeam::Blue,
                                HQMTeam::Blue => HQMTeam::Red
                            };

                            if let HQMOffsideStatus::Warning(team, p, i) = &self.game.offside_status {
                                if *team == touching_team {
                                    let pass_origin = if this_connected_player_index == *i {
                                        puck.body.pos.clone()
                                    } else {
                                        p.clone()
                                    };
                                    self.call_offside(touching_team, &pass_origin);
                                }
                                continue;

                            }
                            if let HQMIcingStatus::Warning(team, p) = &self.game.icing_status {
                                if touching_team != *team {
                                    if faceoff_position == "G" {
                                        self.game.icing_status = HQMIcingStatus::No;
                                        self.add_server_chat_message(String::from("Icing waved off"));
                                    } else {
                                        let copy = p.clone();
                                        self.call_icing(other_team, &copy);
                                    }
                                } else {
                                    self.game.icing_status = HQMIcingStatus::No;
                                    self.add_server_chat_message(String::from("Icing waved off"));
                                }
                            } else if let HQMIcingStatus::NotTouched (_, _) = self.game.icing_status {
                                self.game.icing_status = HQMIcingStatus::No;
                            }
                        }
                    }
                },
                HQMSimulationEvent::PuckEnteredOtherHalf {
                    team, puck
                } => {

                    if let HQMGameObject::Puck(puck) = & self.game.world.objects[puck] {
                        if let Some(touch) = puck.touches.front() {
                            if team == touch.team && self.game.icing_status == HQMIcingStatus::No {
                                self.game.icing_status = HQMIcingStatus::NotTouched(team, touch.puck_pos.clone());
                            }
                        }
                    }
                },
                HQMSimulationEvent::PuckPassedGoalLine {
                    team, puck: _
                } => {

                    if let HQMIcingStatus::NotTouched(icing_team, p) = &self.game.icing_status {
                        if team == *icing_team {
                            match self.config.icing {
                                HQMIcingConfiguration::Touch => {
                                    self.game.icing_status = HQMIcingStatus::Warning(team, p.clone());
                                    self.add_server_chat_message(String::from("Icing warning"));
                                }
                                HQMIcingConfiguration::NoTouch => {
                                    let copy = p.clone();
                                    self.call_icing(team, &copy);
                                }
                                HQMIcingConfiguration::Off => {}
                            }
                        }
                    }
                },
                HQMSimulationEvent::PuckEnteredOffensiveZone {
                    team, puck
                } => {
                    if self.game.offside_status == HQMOffsideStatus::InNeutralZone {
                        if let HQMGameObject::Puck(puck) = & self.game.world.objects[puck] {
                            if let Some(touch) = puck.touches.front() {
                                if team == touch.team &&
                                    has_players_in_offensive_zone(& self.game.world, team) {
                                    match self.config.offside {
                                        HQMOffsideConfiguration::Delayed => {
                                            self.game.offside_status = HQMOffsideStatus::Warning(team, touch.puck_pos.clone(), touch.player_index);
                                            self.add_server_chat_message(String::from("Offside warning"));
                                        }
                                        HQMOffsideConfiguration::Immediate => {
                                            let copy = touch.puck_pos.clone();
                                            self.call_offside(team, &copy);
                                        },
                                        HQMOffsideConfiguration::Off => {
                                            self.game.offside_status = HQMOffsideStatus::InOffensiveZone(team);
                                        }
                                    }
                                } else {
                                    self.game.offside_status = HQMOffsideStatus::InOffensiveZone(team);
                                }
                            } else {
                                self.game.offside_status = HQMOffsideStatus::InOffensiveZone(team);
                            }
                        }
                    }

                },
                HQMSimulationEvent::PuckLeftOffensiveZone {
                    team: _, puck: _
                } => {
                    if let HQMOffsideStatus::Warning(_, _, _) = self.game.offside_status {
                        self.add_server_chat_message(String::from("Offside waved off"));
                    }
                    self.game.offside_status = HQMOffsideStatus::InNeutralZone;

                }
            }
        }
        if let HQMOffsideStatus::Warning(team, _, _) = self.game.offside_status {
            if !has_players_in_offensive_zone(& self.game.world,team) {
                self.game.offside_status = HQMOffsideStatus::InOffensiveZone(team);
                self.add_server_chat_message(String::from("Offside waved off"));
            }
        }
    }

    pub(crate) fn update_clock(&mut self) {
        if !self.game.paused {
            if self.game.period == 0 && self.game.time > 2000 {
                let mut has_red_players = false;
                let mut has_blue_players = false;
                for object in self.game.world.objects.iter() {
                    if let HQMGameObject::Player(skater) = object {
                        match skater.team {
                            HQMTeam::Red => {
                                has_red_players = true;
                            },
                            HQMTeam::Blue => {
                                has_blue_players = true;
                            },
                        }
                    }
                    if has_red_players && has_blue_players {
                        self.game.time = 2000;
                        break;
                    }
                }
            }

            if self.game.time_break > 0 {
                self.game.time_break -= 1;
                if self.game.time_break == 0 {
                    self.game.is_intermission_goal = false;
                    if self.game.game_over {
                        self.new_game();
                    } else {
                        if self.game.time == 0 {
                            self.game.time = self.config.time_period*100;
                        }
                        self.do_faceoff();
                    }

                }
            } else if self.game.time > 0 {
                self.game.time -= 1;
                if self.game.time == 0 {
                    self.game.period += 1;
                    if self.game.period > 3 && self.game.red_score != self.game.blue_score {
                        self.game.time_break = self.config.time_intermission*100;
                        self.game.game_over = true;
                    } else {
                        self.game.time_break = self.config.time_intermission*100;
                        self.game.next_faceoff_spot = self.game.world.rink.center_faceoff_spot.clone();
                    }
                }
            }

        }
    }

    fn do_faceoff(&mut self){
        let faceoff_spot = &self.game.next_faceoff_spot;

        let positions = get_faceoff_positions(& self.players, & self.game.world.objects,
                                                    &self.game.world.rink.allowed_positions);

        let puck_pos = &faceoff_spot.center_position + &(1.5f32*Vector3::y());

        self.game.world.objects = vec![HQMGameObject::None; 32];
        self.game.world.create_puck_object(puck_pos.clone(), Matrix3::identity());

        let mut messages = Vec::new();

        fn setup (messages: & mut Vec<HQMMessage>, world: & mut HQMGameWorld,
                  player: & mut HQMConnectedPlayer, player_index: usize, faceoff_position: String, pos: Point3<f32>, rot: Matrix3<f32>, team: HQMTeam) {
            let new_object_index = world.create_player_object(team,pos, rot, player.hand, player_index, faceoff_position, player.mass);
            player.skater = new_object_index;

            let update = HQMMessage::PlayerUpdate {
                player_name: player.player_name.clone(),
                object: new_object_index.map(|x| (x, team)),
                player_index,

                in_server: true,
            };
            messages.push(update);
        }

        for (player_index, (team, faceoff_position)) in positions {
            if let Some(player) = & mut self.players[player_index] {
                let (player_position, player_rotation) = match team {
                    HQMTeam::Red => {
                        faceoff_spot.red_player_positions[&faceoff_position].clone()
                    }
                    HQMTeam::Blue => {
                        faceoff_spot.blue_player_positions[&faceoff_position].clone()
                    }
                };
                setup (& mut messages, & mut self.game.world, player, player_index, faceoff_position, player_position,
                       player_rotation.matrix().clone_owned(), team)
            }

        }

        let rink = &self.game.world.rink;
        self.game.icing_status = HQMIcingStatus::No;
        self.game.offside_status = if rink.red_lines_and_net.offensive_line.point_past_middle_of_line(&puck_pos) {
            HQMOffsideStatus::InOffensiveZone(HQMTeam::Red)
        } else if rink.blue_lines_and_net.offensive_line.point_past_middle_of_line(&puck_pos) {
            HQMOffsideStatus::InOffensiveZone(HQMTeam::Blue)
        } else {
            HQMOffsideStatus::InNeutralZone
        };

        for message in messages {
            self.add_global_message(message, true);
        }

    }

}

fn has_players_in_offensive_zone (world: & HQMGameWorld, team: HQMTeam) -> bool {
    let line = match team {
        HQMTeam::Red => & world.rink.red_lines_and_net.offensive_line,
        HQMTeam::Blue => & world.rink.blue_lines_and_net.offensive_line,
    };

    for object in world.objects.iter() {
        if let HQMGameObject::Player(skater) = object {
            if skater.team == team {
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

fn get_faceoff_positions (players: & [Option<HQMConnectedPlayer>], objects: & [HQMGameObject], allowed_positions: &[String]) -> HashMap<usize, (HQMTeam, String)> {
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
                red_players.push((player_index, player.preferred_faceoff_position.as_ref()));
            } else if team == Some(HQMTeam::Blue) {
                blue_players.push((player_index, player.preferred_faceoff_position.as_ref()));
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
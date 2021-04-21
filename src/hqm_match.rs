use std::collections::HashMap;

use nalgebra::{Matrix3, Point3, Vector3};

use crate::hqm_game::{HQMGame, HQMGameObject, HQMGameState, HQMGameWorld, HQMIcingStatus, HQMOffsideStatus, HQMSkaterHand, HQMTeam};
use crate::hqm_server::{HQMConnectedPlayer, HQMIcingConfiguration, HQMMatchConfiguration, HQMOffsideConfiguration, HQMServer, HQMServerBehaviour};
use crate::hqm_simulate::HQMSimulationEvent;

use tracing::info;


pub struct HQMMatchBehaviour {
    config: HQMMatchConfiguration
}

impl HQMMatchBehaviour {
    pub fn new (config: HQMMatchConfiguration) -> Self {
        HQMMatchBehaviour {
            config
        }
    }

    fn update_players (server: & mut HQMServer<Self>) {

        let mut chat_messages = vec![];

        let inactive_players: Vec<(usize, String)> = server.players.iter_mut().enumerate().filter_map(|(player_index, player)| {
            if let Some(player) = player {
                player.inactivity += 1;
                if player.inactivity > 500 {
                    Some((player_index, player.player_name.clone()))
                } else {
                    None
                }
            } else {
                None
            }
        }).collect();
        for (player_index, player_name) in inactive_players {
            server.remove_player(player_index);
            info!("{} ({}) timed out", player_name, player_index);
            let chat_msg = format!("{} timed out", player_name);
            chat_messages.push(chat_msg);
        }
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
        let mut num_joining_red = joining_red.len().min(server.config.team_max.saturating_sub(red_player_count));
        let mut num_joining_blue = joining_blue.len().min(server.config.team_max.saturating_sub(blue_player_count));

        if server.behaviour.config.force_team_size_parity {
            num_joining_red = num_joining_red.min(blue_player_count + num_joining_blue + 1 - red_player_count);
            num_joining_blue = num_joining_blue.min(red_player_count + num_joining_red + 1 - blue_player_count);
        }
        for (player_index, player_name) in &joining_red[0..num_joining_red] {
            info!("{} ({}) has joined team {:?}", player_name, player_index, HQMTeam::Red);
            server.move_to_team_spawnpoint(*player_index, HQMTeam::Red, server.behaviour.config.spawn_point);
        }
        for (player_index, player_name) in &joining_blue[0..num_joining_blue] {
            info!("{} ({}) has joined team {:?}", player_name, player_index, HQMTeam::Blue);
            server.move_to_team_spawnpoint(*player_index, HQMTeam::Red, server.behaviour.config.spawn_point);
        }

        for message in chat_messages {
            server.add_server_chat_message(message);
        }
    }

    pub(crate) fn set_team_parity(server: & mut HQMServer<Self>, player_index: usize, rule:&str) {
        if let Some(player) = & server.players[player_index] {
            if player.is_admin{
                match rule {
                    "on" => {
                        server.behaviour.config.force_team_size_parity = true;

                        info!("{} ({}) enabled team size parity",player.player_name, player_index);
                        let msg = format!("Team size parity enabled by {}", player.player_name);

                        server.add_server_chat_message(msg);
                    },
                    "off" => {
                        server.behaviour.config.force_team_size_parity = false;

                        info!("{} ({}) disabled team size parity",player.player_name, player_index);
                        let msg = format!("Team size parity disabled by {}", player.player_name);

                        server.add_server_chat_message(msg);
                    }
                    _ => {}
                }
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    fn cheat_gravity (server: & mut HQMServer<Self>, split: &[&str]) {
        if split.len() >= 2 {
            let gravity = split[1].parse::<f32>();
            if let Ok (gravity) = gravity {
                let converted_gravity = gravity/10000.0;
                server.behaviour.config.physics_config.gravity = converted_gravity;
                server.game.world.physics_config.gravity = converted_gravity;
            }
        }
    }

    fn cheat_mass (server: & mut HQMServer<Self>, split: &[&str]) {
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

    pub(crate) fn cheat(server: & mut HQMServer<Self>, player_index: usize, arg:&str) {
        if let Some(player) = & server.players[player_index] {

            if player.is_admin{
                let split: Vec<&str> = arg.split_whitespace().collect();
                if let Some(&command) = split.get(0) {
                    match command {
                        "mass" => {
                            Self::cheat_mass(server, &split);
                        },
                        "gravity" => {
                            Self::cheat_gravity(server, &split);
                        }
                        _ => {}
                    }
                }

            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    pub(crate) fn set_team_size(server: & mut HQMServer<Self>, player_index: usize, size:&str) {
        if let Some(player) = & server.players[player_index] {
            if player.is_admin{
                if let Ok(new_num) = size.parse::<usize>() {
                    if new_num > 0 && new_num <= 15 {
                        server.config.team_max = new_num;

                        info!("{} ({}) set team size to {}",player.player_name, player_index, new_num);
                        let msg = format!("Team size set to {} by {}", new_num, player.player_name);

                        server.add_server_chat_message(msg);
                    }
                }
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    pub(crate) fn set_icing_rule(server: & mut HQMServer<Self>, player_index: usize, rule:&str) {
        if let Some(player) = & server.players[player_index] {
            if player.is_admin{
                match rule {
                    "on" | "touch" => {
                        server.behaviour.config.icing = HQMIcingConfiguration::Touch;
                        info!("{} ({}) enabled touch icing",player.player_name, player_index);
                        let msg = format!("Touch icing enabled by {}",player.player_name);

                        server.add_server_chat_message(msg);
                    },
                    "notouch" => {
                        server.behaviour.config.icing = HQMIcingConfiguration::NoTouch;
                        info!("{} ({}) enabled no-touch icing",player.player_name, player_index);
                        let msg = format!("No-touch icing enabled by {}",player.player_name);

                        server.add_server_chat_message(msg);
                    },
                    "off" => {
                        server.behaviour.config.icing = HQMIcingConfiguration::Off;
                        info!("{} ({}) disabled icing",player.player_name, player_index);
                        let msg = format!("Icing disabled by {}",player.player_name);

                        server.add_server_chat_message(msg);
                    }
                    _ => {}
                }
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    pub(crate) fn set_offside_rule(server: & mut HQMServer<Self>, player_index: usize, rule:&str) {
        if let Some(player) = & server.players[player_index] {
            if player.is_admin{
                match rule {
                    "on" | "delayed" => {
                        server.behaviour.config.offside = HQMOffsideConfiguration::Delayed;
                        info!("{} ({}) enabled offside", player.player_name, player_index);
                        let msg = format!("Offside enabled by {}",player.player_name);

                        server.add_server_chat_message(msg);
                    },
                    "imm" | "immediate" => {
                        server.behaviour.config.offside = HQMOffsideConfiguration::Immediate;
                        info!("{} ({}) enabled immediate offside", player.player_name, player_index);
                        let msg = format!("Immediate offside enabled by {}",player.player_name);

                        server.add_server_chat_message(msg);
                    },
                    "off" => {
                        server.behaviour.config.offside = HQMOffsideConfiguration::Off;
                        info!("{} ({}) disabled offside",player.player_name, player_index);
                        let msg = format!("Offside disabled by {}",player.player_name);

                        server.add_server_chat_message(msg);
                    }
                    _ => {}
                }
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    pub(crate) fn set_first_to_rule(server: & mut HQMServer<Self>, player_index: usize, size:&str) {
        if let Some(player) = & server.players[player_index] {
            if player.is_admin{
                if let Ok(new_num) = size.parse::<u32>() {
                    server.behaviour.config.first_to = new_num;

                    if new_num > 0 {
                        info!("{} ({}) set first-to-goals rule to {} goals",player.player_name, player_index, new_num);
                        let msg = format!("First-to-goals rule set to {} goals by {}", new_num, player.player_name);
                        server.add_server_chat_message(msg);
                    } else {
                        info!("{} ({}) disabled first-to-goals rule",player.player_name, player_index);
                        let msg = format!("First-to-goals rule disabled by {}", player.player_name);
                        server.add_server_chat_message(msg);
                    }
                }
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    pub(crate) fn set_mercy_rule(server: & mut HQMServer<Self>, player_index: usize, size:&str) {
        if let Some(player) = & server.players[player_index] {
            if player.is_admin{
                if let Ok(new_num) = size.parse::<u32>() {
                    server.behaviour.config.mercy = new_num;

                    if new_num > 0 {
                        info!("{} ({}) set mercy rule to {} goals",player.player_name, player_index, new_num);
                        let msg = format!("Mercy rule set to {} goals by {}", new_num, player.player_name);
                        server.add_server_chat_message(msg);
                    } else {
                        info!("{} ({}) disabled mercy rule",player.player_name, player_index);
                        let msg = format!("Mercy rule disabled by {}", player.player_name);
                        server.add_server_chat_message(msg);
                    }
                }
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    pub(crate) fn faceoff (server: & mut HQMServer<Self>, player_index: usize) {
        if server.game.state != HQMGameState::GameOver {
            if let Some(player) = & server.players[player_index] {
                if player.is_admin{
                    server.game.time_break = 5*100;
                    server.game.paused = false; // Unpause if it's paused as well

                    let msg = format!("Faceoff initiated by {}",player.player_name);
                    info!("{} ({}) initiated faceoff",player.player_name, player_index);
                    server.add_server_chat_message(msg);
                } else {
                    server.admin_deny_message(player_index);
                }
            }
        }
    }

    pub(crate) fn reset_game (server: & mut HQMServer<Self>, player_index: usize) {
        if let Some(player) = & server.players[player_index] {
            if player.is_admin{
                info!("{} ({}) reset game",player.player_name, player_index);
                let msg = format!("Game reset by {}",player.player_name);

                server.new_game();

                server.add_server_chat_message(msg);
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    pub(crate) fn start_game (server: & mut HQMServer<Self>, player_index: usize) {
        if let Some(player) = & server.players[player_index] {
            if player.is_admin {
                if server.game.state == HQMGameState::Warmup {
                    info!("{} ({}) started game", player.player_name, player_index);
                    let msg = format!("Game started by {}", player.player_name);

                    server.game.time = 1;

                    server.add_server_chat_message(msg);
                }
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    pub(crate) fn pause (server: & mut HQMServer<Self>, player_index: usize) {
        if let Some(player) = & server.players[player_index] {
            if player.is_admin{
                server.game.paused=true;
                info!("{} ({}) paused game",player.player_name, player_index);
                let msg = format!("Game paused by {}",player.player_name);
                server.add_server_chat_message(msg);
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    pub(crate) fn unpause (server: & mut HQMServer<Self>, player_index: usize) {
        if let Some(player) = & server.players[player_index] {
            if player.is_admin{
                server.game.paused=false;
                info!("{} ({}) resumed game",player.player_name, player_index);
                let msg = format!("Game resumed by {}",player.player_name);

                server.add_server_chat_message(msg);
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    pub(crate) fn set_clock (server: & mut HQMServer<Self>, input_minutes: u32, input_seconds: u32, player_index: usize) {
        if let Some(player) = & server.players[player_index] {
            if player.is_admin{
                server.game.time = (input_minutes * 60 * 100)+ (input_seconds * 100);

                info!("Clock set to {}:{} by {} ({})", input_minutes, input_seconds, player.player_name, player_index);
                let msg = format!("Clock set by {}", player.player_name);
                server.add_server_chat_message(msg);
            } else {
                server.admin_deny_message(player_index);
            }
        }

    }

    pub(crate) fn set_score (server: & mut HQMServer<Self>, input_team: HQMTeam, input_score: u32, player_index: usize) {
        if let Some(player) = & server.players[player_index] {
            if player.is_admin{
                match input_team {
                    HQMTeam::Red =>{
                        server.game.red_score = input_score;

                        info!("{} ({}) changed red score to {}", player.player_name, player_index, input_score);
                        let msg = format!("Red score changed by {}",player.player_name);
                        server.add_server_chat_message(msg);
                    },
                    HQMTeam::Blue =>{
                        server.game.blue_score = input_score;

                        info!("{} ({}) changed blue score to {}", player.player_name, player_index, input_score);
                        let msg = format!("Blue score changed by {}",player.player_name);
                        server.add_server_chat_message(msg);
                    },
                }
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    pub(crate) fn set_period (server: & mut HQMServer<Self>, input_period: u32, player_index: usize) {
        if let Some(player) = & server.players[player_index] {
            if player.is_admin{

                server.game.period = input_period;

                info!("{} ({}) set period to {}", player.player_name, player_index, input_period);
                let msg = format!("Period set by {}",player.player_name);
                server.add_server_chat_message(msg);

            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    fn call_goal (server: & mut HQMServer<Self>, team: HQMTeam, puck: usize) {
        let (new_score, opponent_score) = match team {
            HQMTeam::Red => {
                server.game.red_score += 1;
                (server.game.red_score, server.game.blue_score)
            }
            HQMTeam::Blue => {
                server.game.blue_score += 1;
                (server.game.blue_score, server.game.red_score)
            }
        };

        server.game.time_break = server.behaviour.config.time_break *100;
        server.game.is_intermission_goal = true;
        server.game.next_faceoff_spot = server.game.world.rink.center_faceoff_spot.clone();

        let game_over = if server.game.period > 3 && server.game.red_score != server.game.blue_score {
            true
        } else if server.behaviour.config.mercy > 0 && new_score.saturating_sub(opponent_score) >= server.behaviour.config.mercy {
            true
        } else if server.behaviour.config.first_to > 0 && new_score >= server.behaviour.config.first_to {
            true
        } else {
            false
        };

        if game_over {
            server.game.time_break = server.behaviour.config.time_intermission*100;
            server.game.game_over = true;
        }

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

        server.add_goal_message(team, goal_scorer_index, assist_index);
    }

    fn call_offside(server: & mut HQMServer<Self>, team: HQMTeam, pass_origin: &Point3<f32>) {

        server.game.next_faceoff_spot = server.game.world.rink.get_offside_faceoff_spot(pass_origin, team);
        server.game.time_break = server.behaviour.config.time_break * 100;
        server.game.offside_status = HQMOffsideStatus::Offside(team);
        server.add_server_chat_message(String::from("Offside"));
    }

    fn call_icing(server: & mut HQMServer<Self>, team: HQMTeam, pass_origin: &Point3<f32>) {
        server.game.next_faceoff_spot = server.game.world.rink.get_icing_faceoff_spot(pass_origin, team);
        server.game.time_break = server.behaviour.config.time_break * 100;
        server.game.icing_status = HQMIcingStatus::Icing(team);
        server.add_server_chat_message(String::from("Icing"));
    }

    pub(crate) fn handle_events (server: & mut HQMServer<Self>, events: Vec<HQMSimulationEvent>) {
        if server.game.offside_status.is_offside()
            || server.game.icing_status.is_icing()
            || server.game.period == 0
            || server.game.time == 0
            || server.game.time_break > 0
            || server.game.paused {
            return;
        }
        for event in events {
            match event {
                HQMSimulationEvent::PuckEnteredNet {
                    team, puck
                } => {
                    match &server.game.offside_status {
                        HQMOffsideStatus::Warning(offside_team, p, _) if *offside_team == team => {
                            let copy = p.clone();
                            Self::call_offside(server, team, &copy);
                        }
                        HQMOffsideStatus::Offside(_) => {},
                        _ => {
                            Self::call_goal(server, team, puck);
                        }
                    }
                },
                HQMSimulationEvent::PuckTouch {
                    player, puck
                } => {
                    // Get connected player index from skater
                    if let HQMGameObject::Player(skater) = & server.game.world.objects[player] {
                        let this_connected_player_index = skater.connected_player_index;
                        let touching_team = skater.team;
                        let faceoff_position = skater.faceoff_position.clone();

                        if let HQMGameObject::Puck(puck) = & mut server.game.world.objects[puck] {
                            puck.add_touch(this_connected_player_index, touching_team, server.game.time);

                            let other_team = match touching_team {
                                HQMTeam::Red => HQMTeam::Blue,
                                HQMTeam::Blue => HQMTeam::Red
                            };

                            if let HQMOffsideStatus::Warning(team, p, i) = &server.game.offside_status {
                                if *team == touching_team {
                                    let pass_origin = if this_connected_player_index == *i {
                                        puck.body.pos.clone()
                                    } else {
                                        p.clone()
                                    };
                                    Self::call_offside(server, touching_team, &pass_origin);
                                }
                                continue;

                            }
                            if let HQMIcingStatus::Warning(team, p) = &server.game.icing_status {
                                if touching_team != *team {
                                    if faceoff_position == "G" {
                                        server.game.icing_status = HQMIcingStatus::No;
                                        server.add_server_chat_message(String::from("Icing waved off"));
                                    } else {
                                        let copy = p.clone();
                                        Self::call_icing(server, other_team, &copy);
                                    }
                                } else {
                                    server.game.icing_status = HQMIcingStatus::No;
                                    server.add_server_chat_message(String::from("Icing waved off"));
                                }
                            } else if let HQMIcingStatus::NotTouched (_, _) = server.game.icing_status {
                                server.game.icing_status = HQMIcingStatus::No;
                            }
                        }
                    }
                },
                HQMSimulationEvent::PuckEnteredOtherHalf {
                    team, puck
                } => {

                    if let HQMGameObject::Puck(puck) = & server.game.world.objects[puck] {
                        if let Some(touch) = puck.touches.front() {
                            if team == touch.team && server.game.icing_status == HQMIcingStatus::No {
                                server.game.icing_status = HQMIcingStatus::NotTouched(team, touch.puck_pos.clone());
                            }
                        }
                    }
                },
                HQMSimulationEvent::PuckPassedGoalLine {
                    team, puck: _
                } => {

                    if let HQMIcingStatus::NotTouched(icing_team, p) = &server.game.icing_status {
                        if team == *icing_team {
                            match server.behaviour.config.icing {
                                HQMIcingConfiguration::Touch => {
                                    server.game.icing_status = HQMIcingStatus::Warning(team, p.clone());
                                    server.add_server_chat_message(String::from("Icing warning"));
                                }
                                HQMIcingConfiguration::NoTouch => {
                                    let copy = p.clone();
                                    Self::call_icing(server, team, &copy);
                                }
                                HQMIcingConfiguration::Off => {}
                            }
                        }
                    }
                },
                HQMSimulationEvent::PuckEnteredOffensiveZone {
                    team, puck
                } => {
                    if server.game.offside_status == HQMOffsideStatus::InNeutralZone {
                        if let HQMGameObject::Puck(puck) = & server.game.world.objects[puck] {
                            if let Some(touch) = puck.touches.front() {
                                if team == touch.team &&
                                    has_players_in_offensive_zone(& server.game.world, team) {
                                    match server.behaviour.config.offside {
                                        HQMOffsideConfiguration::Delayed => {
                                            server.game.offside_status = HQMOffsideStatus::Warning(team, touch.puck_pos.clone(), touch.player_index);
                                            server.add_server_chat_message(String::from("Offside warning"));
                                        }
                                        HQMOffsideConfiguration::Immediate => {
                                            let copy = touch.puck_pos.clone();
                                            Self::call_offside(server, team, &copy);
                                        },
                                        HQMOffsideConfiguration::Off => {
                                            server.game.offside_status = HQMOffsideStatus::InOffensiveZone(team);
                                        }
                                    }
                                } else {
                                    server.game.offside_status = HQMOffsideStatus::InOffensiveZone(team);
                                }
                            } else {
                                server.game.offside_status = HQMOffsideStatus::InOffensiveZone(team);
                            }
                        }
                    }

                },
                HQMSimulationEvent::PuckLeftOffensiveZone {
                    team: _, puck: _
                } => {
                    if let HQMOffsideStatus::Warning(_, _, _) = server.game.offside_status {
                        server.add_server_chat_message(String::from("Offside waved off"));
                    }
                    server.game.offside_status = HQMOffsideStatus::InNeutralZone;

                }
            }
        }
        if let HQMOffsideStatus::Warning(team, _, _) = server.game.offside_status {
            if !has_players_in_offensive_zone(& server.game.world,team) {
                server.game.offside_status = HQMOffsideStatus::InOffensiveZone(team);
                server.add_server_chat_message(String::from("Offside waved off"));
            }
        }
    }

    pub(crate) fn update_clock(server: & mut HQMServer<Self>) {
        if !server.game.paused {
            if server.game.period == 0 && server.game.time > 2000 {
                let mut has_red_players = false;
                let mut has_blue_players = false;
                for object in server.game.world.objects.iter() {
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
                        server.game.time = 2000;
                        break;
                    }
                }
            }

            if server.game.time_break > 0 {
                server.game.time_break -= 1;
                if server.game.time_break == 0 {
                    server.game.is_intermission_goal = false;
                    if server.game.game_over {
                        server.new_game();
                    } else {
                        if server.game.time == 0 {
                            server.game.time = server.behaviour.config.time_period*100;
                        }
                        Self::do_faceoff(server);
                    }

                }
            } else if server.game.time > 0 {
                server.game.time -= 1;
                if server.game.time == 0 {
                    server.game.period += 1;
                    if server.game.period > 3 && server.game.red_score != server.game.blue_score {
                        server.game.time_break = server.behaviour.config.time_intermission*100;
                        server.game.game_over = true;
                    } else {
                        server.game.time_break = server.behaviour.config.time_intermission*100;
                        server.game.next_faceoff_spot = server.game.world.rink.center_faceoff_spot.clone();
                    }
                }
            }

        }
    }

    pub fn clear_pucks(server: & mut HQMServer<Self>) {
        for object in server.game.world.objects.iter_mut() {
            if let HQMGameObject::Puck(_puck) = object {
                *object = HQMGameObject::None;
            }
        }
    }


    fn do_faceoff(server: & mut HQMServer<Self>){
        let positions = get_faceoff_positions(& server.players, & server.game.world.objects,
                                              &server.game.world.rink.allowed_positions);

        Self::clear_pucks(server);

        let puck_pos = &server.game.next_faceoff_spot.center_position + &(1.5f32*Vector3::y());

        server.game.world.create_puck_object(puck_pos, Matrix3::identity());

        for (player_index, (team, faceoff_position)) in positions {
            let (player_position, player_rotation) = match team {
                HQMTeam::Red => {
                    server.game.next_faceoff_spot.red_player_positions[&faceoff_position].clone()
                }
                HQMTeam::Blue => {
                    server.game.next_faceoff_spot.blue_player_positions[&faceoff_position].clone()
                }
            };
            server.move_to_team (player_index, team, player_position, player_rotation);
        }

        let rink = &server.game.world.rink;
        server.game.icing_status = HQMIcingStatus::No;
        server.game.offside_status = if rink.red_lines_and_net.offensive_line.point_past_middle_of_line(&puck_pos) {
            HQMOffsideStatus::InOffensiveZone(HQMTeam::Red)
        } else if rink.blue_lines_and_net.offensive_line.point_past_middle_of_line(&puck_pos) {
            HQMOffsideStatus::InOffensiveZone(HQMTeam::Blue)
        } else {
            HQMOffsideStatus::InNeutralZone
        };

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

impl HQMServerBehaviour for HQMMatchBehaviour {
    fn before_tick(server: &mut HQMServer<Self>) where Self: Sized {
        Self::update_players(server);
    }

    fn after_tick(server: &mut HQMServer<Self>, events: Vec<HQMSimulationEvent>) where Self: Sized {
        Self::handle_events(server, events);
        Self::update_clock(server);
        server.game.update_game_state();
    }

    fn handle_command(server: &mut HQMServer<Self>, command: &str, arg: &str, player_index: usize) where Self: Sized {
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
                                Self::set_icing_rule(server, player_index, arg);
                            }
                        },
                        "offside" => {
                            if let Some(arg) = args.get(1) {
                                Self::set_offside_rule(server, player_index, arg);
                            }
                        },
                        "mercy" => {
                            if let Some(arg) = args.get(1) {
                                Self::set_mercy_rule(server, player_index, arg);
                            }
                        },
                        "first" => {
                            if let Some(arg) = args.get(1) {
                                Self::set_first_to_rule(server, player_index, arg);
                            }
                        }
                        "teamsize" => {
                            if let Some(arg) = args.get(1) {
                                Self::set_team_size(server, player_index, arg);
                            }
                        },
                        "teamparity" => {
                            if let Some(arg) = args.get(1) {
                                Self::set_team_parity(server, player_index, arg);
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
                Self::faceoff(server, player_index);
            },
            "start" | "startgame" => {
                Self::start_game(server, player_index);
            },
            "reset" | "resetgame" => {
                Self::reset_game(server, player_index);
            },
            "pause" | "pausegame" => {
                Self::pause(server, player_index);
            },
            "unpause" | "unpausegame" => {
                Self::unpause(server, player_index);
            },
            "lefty" => {
                server.set_hand(HQMSkaterHand::Left, player_index);
            },
            "righty" => {
                server.set_hand(HQMSkaterHand::Right, player_index);
            },
            "view" => {
                if let Ok(view_player_index) = arg.parse::<usize>() {
                    server.view(view_player_index, player_index);
                }
            },
            "restoreview" => {
                if let Some(player) = & mut server.players[player_index] {
                    if player.view_player_index != player_index {
                        player.view_player_index = player_index;
                        server.add_directed_server_chat_message("View has been restored".to_string(), player_index);
                    }
                }
            },
            "icing" => {
                Self::set_icing_rule (server, player_index, arg);
            },
            "offside" => {
                Self::set_offside_rule (server, player_index, arg);
            },
            "rules" => {
                let offside_str = match server.behaviour.config.offside {
                    HQMOffsideConfiguration::Off => "Offside disabled",
                    HQMOffsideConfiguration::Delayed => "Offside enabled",
                    HQMOffsideConfiguration::Immediate => "Immediate offside enabled"
                };
                let icing_str = match server.behaviour.config.icing {
                    HQMIcingConfiguration::Off => "Icing disabled",
                    HQMIcingConfiguration::Touch => "Icing enabled",
                    HQMIcingConfiguration::NoTouch => "No-touch icing enabled"
                };
                let msg = format!("{}, {}", offside_str, icing_str);
                server.add_directed_server_chat_message(msg, player_index);
            },
            "cheat" => {
                if server.behaviour.config.cheats_enabled {
                    Self::cheat(server, player_index, arg);
                }
            },
            _ => {}
        };

    }

    fn create_game(& mut self, game_id: u32) -> HQMGame {
        let warmup_pucks = self.config.warmup_pucks;
        let mut game = HQMGame::new(game_id, warmup_pucks, self.config.physics_config.clone());
        let puck_line_start= game.world.rink.width / 2.0 - 0.4 * ((warmup_pucks - 1) as f32);

        for i in 0..warmup_pucks {
            let pos = Point3::new(puck_line_start + 0.8*(i as f32), 1.5, game.world.rink.length / 2.0);
            let rot = Matrix3::identity();
            game.world.create_puck_object(pos, rot);
        }
        game.time = self.config.time_warmup * 100;
        game
    }
}

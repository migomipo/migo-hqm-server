use nalgebra::{Point3, Rotation3};
use tracing::info;

use migo_hqm_server::hqm_game::{HQMGame, HQMPhysicsConfiguration, HQMSkaterHand, HQMTeam};
use migo_hqm_server::hqm_server::{
    HQMServer, HQMServerBehaviour, HQMServerPlayerData, HQMSpawnPoint,
};
use migo_hqm_server::hqm_simulate::HQMSimulationEvent;
use std::collections::{HashMap, VecDeque};
use std::f32::consts::PI;
use std::rc::Rc;

pub struct HQMPucksInNetConfiguration {
    pub team_max: usize,
    pub time_period: u32,
    pub time_warmup: u32,
    pub time_intermission: u32,
    pub mercy: u32,
    pub first_to: u32,
    pub pucks: usize,
    pub physics_config: HQMPhysicsConfiguration,
    pub dual_control: bool,
    pub spawn_point: HQMSpawnPoint,
}

pub struct HQMPucksInNetBehaviour {
    pub config: HQMPucksInNetConfiguration,
    pub paused: bool,
    pause_timer: u32,
    puck_respawns: VecDeque<u32>,
    team_switch_timer: HashMap<usize, u32>,
}

impl HQMPucksInNetBehaviour {
    pub fn new(config: HQMPucksInNetConfiguration) -> Self {
        HQMPucksInNetBehaviour {
            config,
            paused: false,
            pause_timer: 0,
            puck_respawns: VecDeque::new(),
            team_switch_timer: Default::default(),
        }
    }

    fn do_faceoff(&mut self, server: &mut HQMServer) {
        let center = server.game.world.rink.width / 2.0;
        let redline = server.game.world.rink.length / 2.0;
        server.game.world.clear_pucks();
        for _ in 0..self.config.pucks {
            let x = center + (rand::random::<f32>() - 0.5) * 10.0;
            let z = redline + (rand::random::<f32>() - 0.5) * 10.0;
            let pos = Point3::new(x, 1.5, z);
            let rot = Rotation3::identity();
            server.game.world.create_puck_object(pos, rot);
        }
        let mut red_players = vec![];
        let mut blue_players = vec![];
        for (player_index, player) in server.players.iter().enumerate() {
            if let Some(player) = player {
                if let Some((_, team)) = player.object {
                    if team == HQMTeam::Red {
                        red_players.push(player_index);
                    } else {
                        blue_players.push(player_index);
                    }
                }
            }
        }
        if !red_players.is_empty() {
            let player_line_start = center - 1.0 * ((red_players.len() - 1) as f32);
            for (i, red_player_index) in red_players.into_iter().enumerate() {
                let pos = Point3::new(player_line_start + 2.0 * (i as f32), 1.5, redline + 10.0);
                let rot = Rotation3::identity();
                server.spawn_skater(red_player_index, HQMTeam::Red, pos, rot);
            }
        }
        if !blue_players.is_empty() {
            let player_line_start = center - 1.0 * ((blue_players.len() - 1) as f32);
            for (i, blue_player_index) in blue_players.into_iter().enumerate() {
                let pos = Point3::new(player_line_start + 2.0 * (i as f32), 1.5, redline - 10.0);
                let rot = Rotation3::from_euler_angles(0.0, PI, 0.0);
                server.spawn_skater(blue_player_index, HQMTeam::Blue, pos, rot);
            }
        }
    }

    fn call_goal(&mut self, server: &mut HQMServer, team: HQMTeam, puck: usize) {
        match team {
            HQMTeam::Red => {
                server.game.red_score += 1;
            }
            HQMTeam::Blue => {
                server.game.blue_score += 1;
            }
        };

        let (goal_scorer_index, assist_index) =
            if let Some(this_puck) = &mut server.game.world.objects.get_puck_mut(puck) {
                let mut goal_scorer_index = None;
                let mut assist_index = None;
                let mut goal_scorer_first_touch = 0;

                for touch in this_puck.touches.iter() {
                    if goal_scorer_index.is_none() {
                        if touch.team == team {
                            goal_scorer_index = Some(touch.player_index);
                            goal_scorer_first_touch = touch.first_time;
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

                (goal_scorer_index, assist_index)
            } else {
                return;
            };

        server.game.world.remove_puck(puck);
        self.puck_respawns.push_back(200);

        let (new_score, opponent_score) = match team {
            HQMTeam::Red => (server.game.red_score, server.game.blue_score),
            HQMTeam::Blue => (server.game.blue_score, server.game.red_score),
        };

        let game_over = if self.config.mercy > 0
            && new_score.saturating_sub(opponent_score) >= self.config.mercy
        {
            true
        } else if self.config.first_to > 0 && new_score >= self.config.first_to {
            true
        } else {
            false
        };

        server.game.goal_message_timer = 200;
        server.add_goal_message(team, goal_scorer_index, assist_index);

        if game_over {
            server.game.game_over = true;
        }

        if server.game.game_over {
            self.pause_timer = 1000;
        }
    }

    fn handle_events(&mut self, server: &mut HQMServer, events: &[HQMSimulationEvent]) {
        for event in events {
            match event {
                HQMSimulationEvent::PuckEnteredNet { team, puck } => {
                    let (team, puck) = (*team, *puck);
                    self.call_goal(server, team, puck);
                }
                HQMSimulationEvent::PuckTouch { player, puck, .. } => {
                    let (player, puck) = (*player, *puck);
                    // Get connected player index from skater
                    if let Some((player_index, touching_team, _)) =
                        server.players.get_from_object_index(player)
                    {
                        if let Some(puck) = server.game.world.objects.get_puck_mut(puck) {
                            puck.add_touch(player_index, touching_team, server.game.time);
                        }
                    }
                }

                _ => {}
            }
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
                );
                add_players_dual_control(
                    joining_blue,
                    server,
                    HQMTeam::Blue,
                    self.config.spawn_point,
                    &mut new_blue_player_count,
                    self.config.team_max,
                );
            } else {
                add_players(
                    joining_red,
                    server,
                    HQMTeam::Red,
                    self.config.spawn_point,
                    &mut new_red_player_count,
                    self.config.team_max,
                );
                add_players(
                    joining_blue,
                    server,
                    HQMTeam::Blue,
                    self.config.spawn_point,
                    &mut new_blue_player_count,
                    self.config.team_max,
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
                    server.game.goal_message_timer = 5 * 100;
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

    fn update_clock(&mut self, server: &mut HQMServer, period_length: u32, intermission_time: u32) {
        if !self.paused {
            if self.pause_timer > 0 {
                self.pause_timer -= 1;
                if self.pause_timer == 0 {
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
                    if server.game.period == 0 {
                        server.game.period += 1;
                        self.pause_timer = intermission_time;
                    } else {
                        server.game.time = 1;
                        if server.game.red_score != server.game.blue_score {
                            server.game.game_over = true;
                            self.pause_timer = intermission_time;
                        }
                    }
                }
            }
            server.game.goal_message_timer = server.game.goal_message_timer.saturating_sub(1);
        }
    }
}

impl HQMServerBehaviour for HQMPucksInNetBehaviour {
    fn before_tick(&mut self, server: &mut HQMServer) {
        self.update_players(server);
    }

    fn after_tick(&mut self, server: &mut HQMServer, events: &[HQMSimulationEvent]) {
        if self.pause_timer > 0
            || server.game.time == 0
            || server.game.game_over
            || server.game.period == 0
            || self.paused
        {
            // Nothing
        } else {
            self.handle_events(server, events);

            let center = server.game.world.rink.width / 2.0;
            let redline = server.game.world.rink.length / 2.0;
            for t in self.puck_respawns.iter_mut() {
                *t -= 1;
                if *t == 0 {
                    let x = center + (rand::random::<f32>() - 0.5) * 10.0;
                    let z = redline + (rand::random::<f32>() - 0.5) * 10.0;
                    let pos = Point3::new(x, 1.5, z);
                    let rot = Rotation3::identity();
                    server.game.world.create_puck_object(pos, rot);
                }
            }
            self.puck_respawns.retain(|x| *x > 0);
        }

        self.update_clock(
            server,
            self.config.time_period * 100,
            self.config.time_intermission * 100,
        );
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
                        "hand" => match args[1] {
                            "left" => {
                                server.set_hand(HQMSkaterHand::Left, player_index);
                            }
                            "right" => {
                                server.set_hand(HQMSkaterHand::Right, player_index);
                            }
                            _ => {}
                        },
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
            _ => {}
        };
    }

    fn create_game(&mut self) -> HQMGame {
        self.paused = false;

        let warmup_pucks = self.config.pucks;

        let mut game = HQMGame::new(warmup_pucks, self.config.physics_config.clone(), 22.83);
        game.history_length = 0;
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
        self.team_switch_timer.remove(&player_index);
    }

    fn after_player_force_off(&mut self, _server: &mut HQMServer, player_index: usize) {
        self.team_switch_timer.insert(player_index, 500);
    }

    fn get_number_of_players(&self) -> u32 {
        self.config.team_max as u32
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

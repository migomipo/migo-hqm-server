use tracing::info;

use migo_hqm_server::hqm_game::{HQMGame, HQMTeam};
use migo_hqm_server::hqm_match_util::{HQMMatch, HQMMatchConfiguration};
use migo_hqm_server::hqm_server::{
    HQMServer, HQMServerBehaviour, HQMServerPlayerIndex, HQMSpawnPoint,
};
use migo_hqm_server::hqm_simulate::HQMSimulationEvent;
use std::collections::HashMap;
use std::rc::Rc;

pub struct HQMMatchBehaviour {
    pub m: HQMMatch,
    pub spawn_point: HQMSpawnPoint,
    pub(crate) team_switch_timer: HashMap<HQMServerPlayerIndex, u32>,
    pub team_max: usize,
}

impl HQMMatchBehaviour {
    pub fn new(config: HQMMatchConfiguration, team_max: usize, spawn_point: HQMSpawnPoint) -> Self {
        HQMMatchBehaviour {
            m: HQMMatch::new(config),
            spawn_point,
            team_switch_timer: Default::default(),
            team_max,
        }
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
                    let has_skater = player.object.is_some();
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
                    let has_skater = player.object.is_some();
                    if has_skater {
                        self.team_switch_timer.insert(player_index, 500);
                        spectating_players.push((player_index, player.player_name.clone()))
                    }
                }
            }
        }
        for (player_index, player_name) in spectating_players {
            info!("{} ({}) is spectating", player_name, player_index);
            server.move_to_spectator(player_index);
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

            for (player_index, player_name) in joining_red {
                add_player(
                    &mut self.m,
                    player_index,
                    player_name,
                    server,
                    HQMTeam::Red,
                    self.spawn_point,
                    &mut new_red_player_count,
                    self.team_max,
                )
            }
            for (player_index, player_name) in joining_blue {
                add_player(
                    &mut self.m,
                    player_index,
                    player_name,
                    server,
                    HQMTeam::Blue,
                    self.spawn_point,
                    &mut new_blue_player_count,
                    self.team_max,
                )
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

    pub(crate) fn force_player_off_ice(
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
                    if server.move_to_spectator(force_player_index) {
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

    pub(crate) fn set_team_size(
        &mut self,
        server: &mut HQMServer,
        player_index: HQMServerPlayerIndex,
        size: &str,
    ) {
        if let Some(player) = server.players.get(player_index) {
            if player.is_admin {
                if let Ok(new_num) = size.parse::<usize>() {
                    if new_num > 0 && new_num <= 15 {
                        self.team_max = new_num;

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
}

impl HQMServerBehaviour for HQMMatchBehaviour {
    fn before_tick(&mut self, server: &mut HQMServer) {
        self.update_players(server);
    }

    fn after_tick(&mut self, server: &mut HQMServer, events: &[HQMSimulationEvent]) {
        self.m.after_tick(server, events);
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
                                self.m
                                    .set_score(server, HQMTeam::Red, input_score, player_index);
                            }
                        }
                        "bluescore" => {
                            if let Ok(input_score) = args[1].parse::<u32>() {
                                self.m
                                    .set_score(server, HQMTeam::Blue, input_score, player_index);
                            }
                        }
                        "period" => {
                            if let Ok(input_period) = args[1].parse::<u32>() {
                                self.m.set_period(server, input_period, player_index);
                            }
                        }
                        "periodnum" => {
                            if let Ok(input_period) = args[1].parse::<u32>() {
                                self.m.set_period_num(server, input_period, player_index);
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
                                    self.m.set_clock(
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
                                self.m.set_icing_rule(server, player_index, arg);
                            }
                        }
                        "offside" => {
                            if let Some(arg) = args.get(1) {
                                self.m.set_offside_rule(server, player_index, arg);
                            }
                        }
                        "twolinepass" => {
                            if let Some(arg) = args.get(1) {
                                self.m.set_twoline_pass(server, player_index, arg);
                            }
                        }
                        "offsideline" => {
                            if let Some(arg) = args.get(1) {
                                self.m.set_offside_line(server, player_index, arg);
                            }
                        }
                        "mercy" => {
                            if let Some(arg) = args.get(1) {
                                self.m.set_mercy_rule(server, player_index, arg);
                            }
                        }
                        "first" => {
                            if let Some(arg) = args.get(1) {
                                self.m.set_first_to_rule(server, player_index, arg);
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
                                self.m.set_goal_replay(server, player_index, arg);
                            }
                        }
                        _ => {}
                    }
                }
            }
            "faceoff" => {
                self.m.faceoff(server, player_index);
            }
            "start" | "startgame" => {
                self.m.start_game(server, player_index);
            }
            "reset" | "resetgame" => {
                self.m.reset_game(server, player_index);
            }
            "pause" | "pausegame" => {
                self.m.pause(server, player_index);
            }
            "unpause" | "unpausegame" => {
                self.m.unpause(server, player_index);
            }
            "sp" | "setposition" => {
                self.m
                    .set_preferred_faceoff_position(server, player_index, arg);
            }
            "fs" => {
                if let Ok(force_player_index) = arg.parse::<HQMServerPlayerIndex>() {
                    self.force_player_off_ice(server, player_index, force_player_index);
                }
            }
            "icing" => {
                self.m.set_icing_rule(server, player_index, arg);
            }
            "offside" => {
                self.m.set_offside_rule(server, player_index, arg);
            }
            "rules" => {
                self.m.msg_rules(server, player_index);
            }
            _ => {}
        };
    }

    fn create_game(&mut self) -> HQMGame {
        self.m.create_game()
    }

    fn before_player_exit(&mut self, _server: &mut HQMServer, player_index: HQMServerPlayerIndex) {
        self.m.cleanup_player(player_index);
        self.team_switch_timer.remove(&player_index);
    }

    fn get_number_of_players(&self) -> u32 {
        self.team_max as u32
    }
}

fn add_player(
    m: &mut HQMMatch,
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
        .spawn_skater_at_spawnpoint(player_index, team, spawn_point)
        .is_some()
    {
        info!(
            "{} ({}) has joined team {:?}",
            player_name, player_index, team
        );
        *player_count += 1;

        m.clear_started_goalie(player_index);
    }
}

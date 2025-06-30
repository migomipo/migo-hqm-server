use tracing::info;

use std::collections::{HashMap, HashSet};

use crate::game::{PhysicsEvent, PlayerId};
use crate::game::{PlayerIndex, Team};
pub use crate::gamemode::match_util::{
    ALLOWED_POSITIONS, IcingConfiguration, Match, MatchConfiguration, OffsideConfiguration,
    OffsideLineConfiguration, TwoLinePassConfiguration,
};
use crate::gamemode::util::{SpawnPoint, add_players, get_spawnpoint};
use crate::gamemode::{ExitReason, GameMode, InitialGameValues, Server, ServerMut, ServerMutParts};

pub struct StandardMatchGameMode {
    pub m: Match,
    pub spawn_point: SpawnPoint,
    pub(crate) team_switch_timer: HashMap<PlayerId, u32>,
    pub(crate) show_extra_messages: HashSet<PlayerId>,
    pub team_max: usize,
}

impl StandardMatchGameMode {
    pub fn new(config: MatchConfiguration, team_max: usize, spawn_point: SpawnPoint) -> Self {
        StandardMatchGameMode {
            m: Match::new(config),
            spawn_point,
            team_switch_timer: Default::default(),
            show_extra_messages: Default::default(),
            team_max,
        }
    }

    fn update_players(&mut self, mut server: ServerMut) {
        let spawn_point = self.spawn_point;
        let ServerMutParts { players, rink, .. } = server.as_mut_parts();
        let rink = &*rink;

        let (red_player_count, blue_player_count) = add_players(
            players,
            self.team_max,
            &mut self.team_switch_timer,
            Some(&self.show_extra_messages),
            |team, _| get_spawnpoint(rink, team, spawn_point),
            |_| {},
            |player_index, _| {
                self.m.clear_started_goalie(player_index);
            },
        );

        let values = server.scoreboard_mut();

        if values.period == 0 && values.time > 2000 && red_player_count > 0 && blue_player_count > 0
        {
            values.time = 2000;
        }
    }

    pub(crate) fn force_player_off_ice(
        &mut self,
        mut server: ServerMut,
        admin_player_id: PlayerId,
        force_player_index: PlayerIndex,
    ) {
        if let Some(player) = server.players_mut().check_admin_or_deny(admin_player_id) {
            let admin_player_name = player.name();

            if let Some(force_player) = server.players().get_by_index(force_player_index) {
                let force_player_id = force_player.id;
                let force_player_name = force_player.name();
                if server.players_mut().move_to_spectator(force_player_id) {
                    let msg = format!("{force_player_name} forced off ice by {admin_player_name}");
                    info!(
                        "{} ({}) forced {} ({}) off ice",
                        admin_player_name, admin_player_id, force_player_name, force_player_id
                    );
                    server.players_mut().add_server_chat_message(msg);
                    self.team_switch_timer.insert(force_player_id, 500);
                }
            }
        }
    }

    pub(crate) fn set_team_size(&mut self, mut server: ServerMut, player_id: PlayerId, size: &str) {
        if let Some(player) = server.players_mut().check_admin_or_deny(player_id) {
            if let Ok(new_num) = size.parse::<usize>() {
                if new_num > 0 && new_num <= 15 {
                    self.team_max = new_num;
                    let name = player.name();

                    info!("{} ({}) set team size to {}", name, player_id, new_num);
                    let msg = format!("Team size set to {new_num} by {name}");

                    server.players_mut().add_server_chat_message(msg);
                }
            }
        }
    }
}

impl GameMode for StandardMatchGameMode {
    fn init(&mut self, mut server: ServerMut) {
        server.replay_mut().set_history_length(1000)
    }

    fn before_tick(&mut self, server: ServerMut) {
        self.update_players(server);
    }

    fn after_tick(&mut self, server: ServerMut, events: &[PhysicsEvent]) {
        self.m.after_tick(server, events);
    }

    fn handle_command(
        &mut self,
        mut server: ServerMut,
        command: &str,
        arg: &str,
        player_id: PlayerId,
    ) {
        match command {
            "set" => {
                let args = arg.split(" ").collect::<Vec<&str>>();
                if args.len() > 1 {
                    match args[0] {
                        "redscore" => {
                            if let Ok(input_score) = args[1].parse::<u32>() {
                                self.m.set_score(server, Team::Red, input_score, player_id);
                            }
                        }
                        "bluescore" => {
                            if let Ok(input_score) = args[1].parse::<u32>() {
                                self.m.set_score(server, Team::Blue, input_score, player_id);
                            }
                        }
                        "period" => {
                            if let Ok(input_period) = args[1].parse::<u32>() {
                                self.m.set_period(server, input_period, player_id);
                            }
                        }
                        "periodnum" => {
                            if let Ok(input_period) = args[1].parse::<u32>() {
                                self.m.set_period_num(server, input_period, player_id);
                            }
                        }
                        "clock" => {
                            let time_part_string = match args[1].parse::<String>() {
                                Ok(time_part_string) => time_part_string,
                                Err(_) => {
                                    return;
                                }
                            };

                            fn parse_t(
                                s: &str,
                            ) -> Result<(u32, u32, u32), std::num::ParseIntError>
                            {
                                let (time_minutes, rest) =
                                    if let Some((time_minutes, rest)) = s.split_once(':') {
                                        (time_minutes.parse::<u32>()?, rest)
                                    } else {
                                        (0, s)
                                    };
                                let (time_seconds, time_centis) =
                                    if let Some((time_seconds, time_centis)) = rest.split_once(".")
                                    {
                                        let mut centis = time_centis.parse::<u32>()?;
                                        if time_centis.len() == 1 {
                                            centis *= 10;
                                        }
                                        (time_seconds.parse::<u32>()?, centis)
                                    } else {
                                        (rest.parse::<u32>()?, 0)
                                    };
                                Ok((time_minutes, time_seconds, time_centis))
                            }

                            if let Ok((time_minutes, time_seconds, time_centis)) =
                                parse_t(&time_part_string)
                            {
                                self.m.set_clock(
                                    server,
                                    (time_minutes * 100 * 60) + (time_seconds * 100) + time_centis,
                                    player_id,
                                );
                            }
                        }
                        "icing" => {
                            if let Some(arg) = args.get(1) {
                                self.m.set_icing_rule(server, player_id, arg);
                            }
                        }
                        "offside" => {
                            if let Some(arg) = args.get(1) {
                                self.m.set_offside_rule(server, player_id, arg);
                            }
                        }
                        "twolinepass" => {
                            if let Some(arg) = args.get(1) {
                                self.m.set_twoline_pass(server, player_id, arg);
                            }
                        }
                        "offsideline" => {
                            if let Some(arg) = args.get(1) {
                                self.m.set_offside_line(server, player_id, arg);
                            }
                        }
                        "mercy" => {
                            if let Some(arg) = args.get(1) {
                                self.m.set_mercy_rule(server, player_id, arg);
                            }
                        }
                        "first" => {
                            if let Some(arg) = args.get(1) {
                                self.m.set_first_to_rule(server, player_id, arg);
                            }
                        }
                        "teamsize" => {
                            if let Some(arg) = args.get(1) {
                                self.set_team_size(server, player_id, arg);
                            }
                        }
                        "goalreplay" => {
                            if let Some(arg) = args.get(1) {
                                self.m.set_goal_replay(server, player_id, arg);
                            }
                        }
                        "spawnoffset" => {
                            if let Ok(rule) = args[1].parse::<f32>() {
                                self.m.set_spawn_offset(server, player_id, rule);
                            }
                        }
                        "spawnplayeraltitude" => {
                            if let Ok(rule) = args[1].parse::<f32>() {
                                self.m.set_spawn_player_altitude(server, player_id, rule);
                            }
                        }
                        "spawnpuckaltitude" => {
                            if let Ok(rule) = args[1].parse::<f32>() {
                                self.m.set_spawn_puck_altitude(server, player_id, rule);
                            }
                        }
                        "spawnplayerkeepstick" => {
                            if let Some(arg) = args.get(1) {
                                self.m.set_spawn_keep_stick(server, player_id, arg);
                            }
                        }
                        _ => {}
                    }
                }
            }
            "faceoff" => {
                self.m.faceoff(server, player_id);
            }
            "start" | "startgame" => {
                self.m.start_game(server, player_id);
            }
            "reset" | "resetgame" => {
                self.m.reset_game(server, player_id);
            }
            "pause" | "pausegame" => {
                self.m.pause(server, player_id);
            }
            "unpause" | "unpausegame" => {
                self.m.unpause(server, player_id);
            }
            "sp" | "setposition" => {
                self.m
                    .set_preferred_faceoff_position(server, player_id, arg);
            }
            "fs" => {
                if let Ok(force_player_index) = arg.parse::<PlayerIndex>() {
                    self.force_player_off_ice(server, player_id, force_player_index);
                }
            }
            "icing" => {
                self.m.set_icing_rule(server, player_id, arg);
            }
            "offside" => {
                self.m.set_offside_rule(server, player_id, arg);
            }
            "rules" => {
                self.m.msg_rules(server, player_id);
            }
            "chatextend" => {
                if arg.eq_ignore_ascii_case("true") || arg.eq_ignore_ascii_case("on") {
                    if self.show_extra_messages.insert(player_id) {
                        server.players_mut().add_directed_server_chat_message(
                            "Team change messages activated",
                            player_id,
                        );
                    }
                } else if (arg.eq_ignore_ascii_case("false") || arg.eq_ignore_ascii_case("off"))
                    && self.show_extra_messages.remove(&player_id)
                {
                    server.players_mut().add_directed_server_chat_message(
                        "Team change messages de-activated",
                        player_id,
                    );
                }
            }
            _ => {}
        };
    }

    fn get_initial_game_values(&mut self) -> InitialGameValues {
        self.m.get_initial_game_values()
    }

    fn game_started(&mut self, server: ServerMut) {
        self.m.game_started(server);
    }

    fn before_player_exit(&mut self, _server: ServerMut, player_id: PlayerId, _reason: ExitReason) {
        self.m.cleanup_player(player_id);
        self.team_switch_timer.remove(&player_id);
        self.show_extra_messages.remove(&player_id);
    }

    fn server_list_team_size(&self) -> u32 {
        self.team_max as u32
    }

    fn include_tick_in_recording(&self, server: Server) -> bool {
        server.scoreboard().period > 0
    }
}

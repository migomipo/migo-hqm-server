use crate::hqm_server::{HQMServerBehaviour, HQMServer, HQMSpawnPoint};
use crate::hqm_simulate::HQMSimulationEvent;
use crate::hqm_game::{HQMPhysicsConfiguration, HQMTeam, HQMGame};
use nalgebra::{Point3, Matrix3};

use tracing::info;

pub struct HQMPermanentWarmup {
    physics_config: HQMPhysicsConfiguration,
    pucks: usize,
    spawn_point: HQMSpawnPoint
}

impl HQMPermanentWarmup {

    pub fn new (physics_config: HQMPhysicsConfiguration, pucks: usize, spawn_point: HQMSpawnPoint) -> Self {
        HQMPermanentWarmup {
            physics_config,
            pucks,
            spawn_point
        }
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


        for (player_index, player_name) in joining_red {
            info!("{} ({}) has joined team {:?}", player_name, player_index, HQMTeam::Red);
            server.move_to_team_spawnpoint(player_index, HQMTeam::Red, self.spawn_point);
        }
        for (player_index, player_name) in joining_blue {
            info!("{} ({}) has joined team {:?}", player_name, player_index, HQMTeam::Blue);
            server.move_to_team_spawnpoint(player_index, HQMTeam::Blue, self.spawn_point);
        }

    }
}

impl HQMServerBehaviour for HQMPermanentWarmup {
    fn before_tick(& mut self, server: &mut HQMServer) {
        self.update_players(server);
    }

    fn after_tick(& mut self, _server: &mut HQMServer, _events: &[HQMSimulationEvent])  {
        // Nothing
    }

    fn handle_command(& mut self, server: &mut HQMServer, cmd: &str, arg: &str, player_index: usize) {
        match cmd {
            "view" => {
                if let Ok(view_player_index) = arg.parse::<usize>() {
                    server.view(view_player_index, player_index);
                }
            },
            "views" => {
                if let Some((view_player_index, _name)) = server.player_exact_unique_match(arg) {
                    server.view(view_player_index, player_index);
                } else {
                    let matches = server.player_search(arg);
                    if matches.is_empty() {
                        server.add_directed_server_chat_message("No matches found", player_index);
                    } else if matches.len() > 1 {
                        server.add_directed_server_chat_message("Multiple matches found, use /view X", player_index);
                        for (found_player_index, found_player_name) in matches.into_iter().take(5) {
                            let str = format!("{}: {}", found_player_index, found_player_name);
                            server.add_directed_server_chat_message(&str, player_index);
                        }
                    } else {
                        server.view(matches[0].0, player_index);
                    }
                }
            }
            "restoreview" => {
                if let Some(player) = & mut server.players[player_index] {
                    if player.view_player_index != player_index {
                        player.view_player_index = player_index;
                        server.add_directed_server_chat_message("View has been restored", player_index);
                    }
                }
            },
            _ => {}
        }
    }

    fn create_game(&mut self, game_id: u32) -> HQMGame where Self: Sized {
        let warmup_pucks = self.pucks;
        let mut game = HQMGame::new(game_id, warmup_pucks, self.physics_config.clone());
        let puck_line_start= game.world.rink.width / 2.0 - 0.4 * ((warmup_pucks - 1) as f32);

        for i in 0..warmup_pucks {
            let pos = Point3::new(puck_line_start + 0.8*(i as f32), 1.5, game.world.rink.length / 2.0);
            let rot = Matrix3::identity();
            game.world.create_puck_object(pos, rot);
        }
        game.time = 30000; // Permanently locked to 5 minutes
        game
    }
}
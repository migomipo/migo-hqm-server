use crate::hqm_server::{HQMServerBehaviour, HQMServer, HQMSpawnPoint};
use crate::hqm_simulate::HQMSimulationEvent;
use crate::hqm_game::{HQMSkaterHand, HQMPhysicsConfiguration, HQMTeam, HQMGame};
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


        for (player_index, player_name) in joining_red {
            info!("{} ({}) has joined team {:?}", player_name, player_index, HQMTeam::Red);
            server.move_to_team_spawnpoint(player_index, HQMTeam::Red, server.behaviour.spawn_point);
        }
        for (player_index, player_name) in joining_blue {
            info!("{} ({}) has joined team {:?}", player_name, player_index, HQMTeam::Blue);
            server.move_to_team_spawnpoint(player_index, HQMTeam::Blue, server.behaviour.spawn_point);
        }

        for message in chat_messages {
            server.add_server_chat_message(message);
        }
    }
}

impl HQMServerBehaviour for HQMPermanentWarmup {
    fn before_tick(server: &mut HQMServer<Self>) where Self: Sized {
        Self::update_players(server);
    }

    fn after_tick(_server: &mut HQMServer<Self>, _events: Vec<HQMSimulationEvent>) where Self: Sized {
        // Nothing
    }

    fn handle_command(server: &mut HQMServer<Self>, cmd: &str, _arg: &str, player_index: usize) where Self: Sized {
        match cmd {
            "lefty" => {
                server.set_hand(HQMSkaterHand::Left, player_index);
            },
            "righty" => {
                server.set_hand(HQMSkaterHand::Right, player_index);
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
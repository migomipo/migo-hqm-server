use migo_hqm_server::hqm_game::{HQMGame, HQMPhysicsConfiguration, HQMTeam};
use migo_hqm_server::hqm_server::{HQMServer, HQMServerBehaviour, HQMServerPlayerIndex};
use migo_hqm_server::hqm_simulate::HQMSimulationEvent;
use nalgebra::{Point3, Rotation3, Vector3};

pub struct HQMFaceoffPracticeBehaviour {
    timer: u32,
    wait_timer: u32,
    physics_config: HQMPhysicsConfiguration,
}

impl HQMFaceoffPracticeBehaviour {
    pub(crate) fn new(physics_config: HQMPhysicsConfiguration) -> Self {
        Self {
            timer: 0,
            wait_timer: 0,
            physics_config,
        }
    }

    fn start_new_round(&mut self, server: &mut HQMServer, player_index: HQMServerPlayerIndex) {
        self.timer = 0;
        server.game.world.clear_pucks();

        let center_x = 15.0;
        let center_z = 30.5;
        let center_pos = Point3::new(center_x, 0.0, center_z);

        let pos = center_pos + Vector3::new(0.0, 1.5, 2.75);
        let rot = Rotation3::identity();
        server.spawn_skater(player_index, HQMTeam::Red, pos, rot);

        let pos = center_pos + Vector3::new(0.0, 1.5, 0.0);
        let rot = Rotation3::identity();
        server.game.world.create_puck_object(pos, rot);
    }
}
impl HQMServerBehaviour for HQMFaceoffPracticeBehaviour {
    fn before_tick(&mut self, server: &mut HQMServer) {
        let mut has_player_already = None;
        let mut wants_to_play = None;
        for (player_index, player) in server.players.iter() {
            if let Some(player) = player {
                if player.object.is_some() {
                    has_player_already = Some(player_index);
                    break;
                } else if player.input.join_red() && wants_to_play.is_none() {
                    wants_to_play = Some(player_index);
                }
            }
        }
        if let Some(player_index) = has_player_already {
            if let Some(player) = server.players.get(player_index) {
                if player.input.spectate() {
                    server.move_to_spectator(player_index);
                    has_player_already = None;
                }
            }
        }

        if let Some(player_index) = has_player_already {
            if self.wait_timer > 0 {
                self.wait_timer -= 1;
                if self.wait_timer == 0 {
                    self.start_new_round(server, player_index);
                }
            } else {
                self.timer += 1;
            }
        } else {
            server.game.world.clear_pucks();
            if let Some(wants_to_play) = wants_to_play {
                let center_x = 15.0;
                let center_z = 30.5;

                let pos = Point3::new(center_x, 0.0, center_z) + Vector3::new(0.0, 1.5, 2.75);
                let rot = Rotation3::identity();
                server.spawn_skater(wants_to_play, HQMTeam::Red, pos, rot);
                self.wait_timer = 300;
            }
        }
    }

    fn after_tick(&mut self, server: &mut HQMServer, events: &[HQMSimulationEvent]) {
        if self.wait_timer == 0 {
            let any_touch = events
                .iter()
                .filter_map(|x| {
                    if let HQMSimulationEvent::PuckTouch { player, .. } = x {
                        Some(*player)
                    } else {
                        None
                    }
                })
                .next();
            if let Some(touch) = any_touch {
                let seconds = self.timer / 100;
                let centi = self.timer % 100;

                if let Some(player_name) = server
                    .players
                    .get_from_object_index(touch)
                    .map(|(_, _, player)| player.player_name.as_str())
                {
                    tracing::info!(
                        "{} touched puck in {}.{:02} seconds ",
                        player_name,
                        seconds,
                        centi
                    )
                }

                let s = format!("{}.{:02} seconds ", seconds, centi);

                server.messages.add_server_chat_message(s);

                self.wait_timer = 300;
            }
        }
    }

    fn create_game(&mut self) -> HQMGame {
        let mut game = HQMGame::new(1, self.physics_config.clone(), -10.0);

        game.time = 30000; // Permanently locked to 5 minutes
        game
    }

    fn get_number_of_players(&self) -> u32 {
        0
    }
}

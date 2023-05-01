use migo_hqm_server::hqm_game::{HQMGame, HQMPhysicsConfiguration, HQMTeam};
use migo_hqm_server::hqm_server::{
    HQMServer, HQMServerBehaviour, HQMServerPlayerIndex, HQMSpawnPoint,
};
use migo_hqm_server::hqm_simulate::HQMSimulationEvent;
use nalgebra::{Point3, Rotation3};

pub struct HQMPermanentWarmup {
    physics_config: HQMPhysicsConfiguration,
    pucks: usize,
    spawn_point: HQMSpawnPoint,
}

impl HQMPermanentWarmup {
    pub fn new(
        physics_config: HQMPhysicsConfiguration,
        pucks: usize,
        spawn_point: HQMSpawnPoint,
    ) -> Self {
        HQMPermanentWarmup {
            physics_config,
            pucks,
            spawn_point,
        }
    }
    fn update_players(&mut self, server: &mut HQMServer) {
        let mut spectating_players = vec![];
        let mut joining_team = vec![];
        for (player_index, player) in server.players.iter() {
            if let Some(player) = player {
                let has_skater = player.object.is_some();
                if has_skater && player.input.spectate() {
                    spectating_players.push(player_index);
                } else if !has_skater {
                    if player.input.join_red() {
                        joining_team.push((player_index, HQMTeam::Red));
                    } else if player.input.join_blue() {
                        joining_team.push((player_index, HQMTeam::Blue));
                    }
                }
            }
        }
        for player_index in spectating_players {
            server.move_to_spectator(player_index);
        }

        fn internal_add(
            server: &mut HQMServer,
            player_index: HQMServerPlayerIndex,
            team: HQMTeam,
            spawn_point: HQMSpawnPoint,
        ) {
            server.spawn_skater_at_spawnpoint(player_index, team, spawn_point);
        }

        for (player_index, team) in joining_team {
            internal_add(server, player_index, team, self.spawn_point);
        }
    }
}

impl HQMServerBehaviour for HQMPermanentWarmup {
    fn before_tick(&mut self, server: &mut HQMServer) {
        self.update_players(server);
    }

    fn after_tick(&mut self, _server: &mut HQMServer, _events: &[HQMSimulationEvent]) {
        // Nothing
    }

    fn handle_command(
        &mut self,
        _server: &mut HQMServer,
        _cmd: &str,
        _arg: &str,
        _player_index: HQMServerPlayerIndex,
    ) {
    }

    fn create_game(&mut self) -> HQMGame
    where
        Self: Sized,
    {
        let warmup_pucks = self.pucks;
        let mut game = HQMGame::new(warmup_pucks, self.physics_config.clone(), -10.0);
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
        game.time = 30000; // Permanently locked to 5 minutes
        game
    }

    fn get_number_of_players(&self) -> u32 {
        0
    }
}

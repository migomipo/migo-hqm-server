use migo_hqm_server::hqm_behaviour::HQMServerBehaviour;
use migo_hqm_server::hqm_game::HQMPhysicsConfiguration;
use migo_hqm_server::hqm_match_util::{get_spawnpoint, HQMSpawnPoint};
use migo_hqm_server::hqm_server::{HQMInitialGameValues, HQMServer, HQMServerPlayerIndex, HQMTeam};
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
        for player_index in spectating_players {
            server.move_to_spectator(player_index);
        }

        fn internal_add(
            server: &mut HQMServer,
            player_index: HQMServerPlayerIndex,
            team: HQMTeam,
            spawn_point: HQMSpawnPoint,
        ) {
            let (pos, rot) = get_spawnpoint(&server.world.rink, team, spawn_point);

            server.spawn_skater(player_index, team, pos, rot);
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

    fn get_number_of_players(&self) -> u32 {
        0
    }

    fn get_initial_game_values(&mut self) -> HQMInitialGameValues {
        let warmup_pucks = self.pucks;

        HQMInitialGameValues {
            values: Default::default(),
            puck_slots: warmup_pucks,
            physics_configuration: self.physics_config.clone(),
            blue_line: -10.0,
        }
    }

    fn game_started(&mut self, server: &mut HQMServer) {
        let warmup_pucks = self.pucks;
        let puck_line_start = server.world.rink.width / 2.0 - 0.4 * ((warmup_pucks - 1) as f32);

        for i in 0..warmup_pucks {
            let pos = Point3::new(
                puck_line_start + 0.8 * (i as f32),
                1.5,
                server.world.rink.length / 2.0,
            );
            let rot = Rotation3::identity();
            server.world.create_puck_object(pos, rot);
        }
    }
}

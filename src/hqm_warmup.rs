use crate::hqm_behaviour_extra::HQMDualControlSetting;
use migo_hqm_server::hqm_game::{HQMGame, HQMPhysicsConfiguration, HQMTeam};
use migo_hqm_server::hqm_server::{
    HQMServer, HQMServerBehaviour, HQMServerPlayerData, HQMSpawnPoint,
};
use migo_hqm_server::hqm_simulate::HQMSimulationEvent;
use nalgebra::{Point3, Rotation3};

pub struct HQMPermanentWarmup {
    physics_config: HQMPhysicsConfiguration,
    pucks: usize,
    spawn_point: HQMSpawnPoint,
    use_dual_control: HQMDualControlSetting,
}

impl HQMPermanentWarmup {
    pub fn new(
        physics_config: HQMPhysicsConfiguration,
        pucks: usize,
        spawn_point: HQMSpawnPoint,
        use_dual_control: HQMDualControlSetting,
    ) -> Self {
        HQMPermanentWarmup {
            physics_config,
            pucks,
            spawn_point,
            use_dual_control,
        }
    }
    fn update_players(&mut self, server: &mut HQMServer) {
        let mut spectating_players = vec![];
        let mut joining_team = vec![];
        for (player_index, player) in server.players.iter().enumerate() {
            if let Some(player) = player {
                let has_skater = player.object.is_some()
                    || server.get_dual_control_player(player_index).is_some();
                if has_skater && player.input.spectate() {
                    spectating_players.push(player_index);
                } else if !has_skater {
                    let dual_control = self.use_dual_control == HQMDualControlSetting::Yes
                        || (self.use_dual_control == HQMDualControlSetting::Combined
                            && player.input.shift());
                    if player.input.join_red() {
                        joining_team.push((player_index, HQMTeam::Red, dual_control));
                    } else if player.input.join_blue() {
                        joining_team.push((player_index, HQMTeam::Blue, dual_control));
                    }
                }
            }
        }
        for player_index in spectating_players {
            server.remove_player_from_dual_control(player_index);
            server.move_to_spectator(player_index);
        }

        fn internal_add(
            server: &mut HQMServer,
            player_index: usize,
            team: HQMTeam,
            spawn_point: HQMSpawnPoint,
        ) {
            server.spawn_skater_at_spawnpoint(player_index, team, spawn_point);
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

        fn internal_add_dual_control(
            server: &mut HQMServer,
            player_index: usize,
            team: HQMTeam,
            spawn_point: HQMSpawnPoint,
        ) {
            let current_empty = find_empty_dual_control(server, team);

            match current_empty {
                Some((index, movement @ Some(_), None)) => {
                    server.update_dual_control(index, movement, Some(player_index));
                }
                Some((index, None, stick @ Some(_))) => {
                    server.update_dual_control(index, Some(player_index), stick);
                }
                _ => {
                    server.spawn_dual_control_skater_at_spawnpoint(
                        team,
                        spawn_point,
                        Some(player_index),
                        None,
                    );
                }
            }
        }
        for (player_index, team, dual_control) in joining_team {
            if dual_control {
                internal_add_dual_control(server, player_index, team, self.spawn_point);
            } else {
                internal_add(server, player_index, team, self.spawn_point);
            }
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
        _player_index: usize,
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

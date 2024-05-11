use crate::game::PhysicsEvent;
use crate::game::{PlayerIndex, PuckObject};
use crate::gamemode::util::{add_players, get_spawnpoint, SpawnPoint};
use crate::gamemode::{GameMode, InitialGameValues, ServerMut, ServerMutParts};
use nalgebra::{Point3, Rotation3};
use std::collections::HashMap;

pub struct PermanentWarmup {
    pucks: usize,
    spawn_point: SpawnPoint,
    team_switch_timer: HashMap<PlayerIndex, u32>,
}

impl PermanentWarmup {
    pub fn new(pucks: usize, spawn_point: SpawnPoint) -> Self {
        PermanentWarmup {
            pucks,
            spawn_point,
            team_switch_timer: Default::default(),
        }
    }
    fn update_players(&mut self, mut server: ServerMut) {
        let spawn_point = self.spawn_point;
        let ServerMutParts { state, rink, .. } = server.as_mut_parts();
        let rink = &*rink;
        add_players(
            state,
            usize::MAX,
            &mut self.team_switch_timer,
            None,
            |team, _| get_spawnpoint(rink, team, spawn_point),
            |_| {},
            |_, _| {},
        );
    }
}

impl GameMode for PermanentWarmup {
    fn before_tick(&mut self, server: ServerMut) {
        self.update_players(server);
    }

    fn after_tick(&mut self, _server: ServerMut, _events: &[PhysicsEvent]) {
        // Nothing
    }

    fn handle_command(
        &mut self,
        _server: ServerMut,
        _cmd: &str,
        _arg: &str,
        _player_index: PlayerIndex,
    ) {
    }

    fn get_initial_game_values(&mut self) -> InitialGameValues {
        let warmup_pucks = self.pucks;

        InitialGameValues {
            values: Default::default(),
            puck_slots: warmup_pucks,
        }
    }

    fn game_started(&mut self, mut server: ServerMut) {
        let warmup_pucks = self.pucks;
        let rink = server.rink();
        let width = rink.width;
        let length = rink.length;
        let puck_line_start = width / 2.0 - 0.4 * ((warmup_pucks - 1) as f32);

        for i in 0..warmup_pucks {
            let pos = Point3::new(puck_line_start + 0.8 * (i as f32), 1.5, length / 2.0);
            let rot = Rotation3::identity();
            server.state_mut().spawn_puck(PuckObject::new(pos, rot));
        }
    }

    fn server_list_team_size(&self) -> u32 {
        0
    }
}

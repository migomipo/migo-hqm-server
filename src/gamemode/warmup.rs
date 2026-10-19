use crate::game::{PhysicsEvent, PlayerId};
use crate::game::{Puck, ScoreboardValues};
use crate::gamemode::util::{SpawnPoint, TeamSelection, get_spawnpoint};
use crate::gamemode::{ExitReason, GameMode, ServerMut};
use glam::Vec3;
use glamx::Rot3;

pub struct PermanentWarmup {
    pucks: usize,
    spawn_point: SpawnPoint,
    teams: TeamSelection,
}

impl PermanentWarmup {
    pub fn new(pucks: usize, spawn_point: SpawnPoint) -> Self {
        PermanentWarmup {
            pucks,
            spawn_point,
            teams: TeamSelection::new(usize::MAX).with_team_switch_cooldown(0),
        }
    }
    fn update_players(&mut self, mut server: ServerMut) {
        let spawn_point = self.spawn_point;
        self.teams.update(&mut server.context(), |rink, team, _| {
            get_spawnpoint(rink, team, spawn_point)
        });
    }
}

impl GameMode for PermanentWarmup {
    fn before_tick(&mut self, server: ServerMut) {
        self.update_players(server);
    }

    fn after_tick(&mut self, _server: ServerMut, _events: &[PhysicsEvent]) -> ScoreboardValues {
        ScoreboardValues::default()
    }

    fn handle_command(
        &mut self,
        _server: ServerMut,
        _cmd: &str,
        _arg: &str,
        _player_index: PlayerId,
    ) {
    }

    fn game_started(&mut self, mut server: ServerMut) {
        let warmup_pucks = self.pucks;
        let rink = server.rink();
        let width = rink.width;
        let length = rink.length;
        let puck_line_start = width / 2.0 - 0.4 * ((warmup_pucks - 1) as f32);

        for i in 0..warmup_pucks {
            let pos = Vec3::new(puck_line_start + 0.8 * (i as f32), 1.5, length / 2.0);
            let rot = Rot3::IDENTITY;
            server
                .state_mut()
                .objects_mut()
                .spawn_puck(Puck::new(pos, rot));
        }
    }

    fn before_player_exit(&mut self, _server: ServerMut, player_id: PlayerId, _reason: ExitReason) {
        self.teams.remove_player(player_id);
    }

    fn server_list_team_size(&self) -> u32 {
        0
    }
}

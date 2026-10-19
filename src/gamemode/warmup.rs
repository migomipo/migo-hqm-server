use crate::game::{PhysicsEvent, PlayerId};
use crate::game::{Puck, ScoreboardValues};
use crate::gamemode::util::{SpawnPoint, TeamSelection, get_spawnpoint};
use crate::gamemode::{ExitReason, GameContext, GameMode};
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
    fn update_players(&mut self, ctx: &mut GameContext) {
        let spawn_point = self.spawn_point;
        self.teams
            .update(ctx, |rink, team, _| get_spawnpoint(rink, team, spawn_point));
    }
}

impl GameMode for PermanentWarmup {
    fn before_tick(&mut self, ctx: &mut GameContext) {
        self.update_players(ctx);
    }

    fn after_tick(&mut self, _ctx: &mut GameContext, _events: &[PhysicsEvent]) -> ScoreboardValues {
        ScoreboardValues::default()
    }

    fn handle_command(
        &mut self,
        _ctx: &mut GameContext,
        _cmd: &str,
        _arg: &str,
        _player_index: PlayerId,
    ) {
    }

    fn game_started(&mut self, ctx: &mut GameContext) {
        let warmup_pucks = self.pucks;
        let rink = &*ctx.rink;
        let width = rink.width;
        let length = rink.length;
        let puck_line_start = width / 2.0 - 0.4 * ((warmup_pucks - 1) as f32);

        for i in 0..warmup_pucks {
            let pos = Vec3::new(puck_line_start + 0.8 * (i as f32), 1.5, length / 2.0);
            let rot = Rot3::IDENTITY;
            ctx.objects.spawn_puck(Puck::new(pos, rot));
        }
    }

    fn before_player_exit(
        &mut self,
        _ctx: &mut GameContext,
        player_id: PlayerId,
        _reason: ExitReason,
    ) {
        self.teams.remove_player(player_id);
    }

    fn server_list_team_size(&self) -> u32 {
        0
    }
}

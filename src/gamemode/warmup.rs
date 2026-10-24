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

    fn game_started(&mut self, ctx: &mut GameContext) {
        let warmup_pucks = self.pucks;
        let rink = &*ctx.rink;
        let width = rink.width;
        let length = rink.length;
        let puck_line_start = width / 2.0 - 0.4 * warmup_pucks.saturating_sub(1) as f32;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::Team;
    use crate::server::test_server::TestServer;

    #[test]
    fn warmup_has_no_commands() {
        let mut t = TestServer::new(PermanentWarmup::new(1, SpawnPoint::Center));
        let player = t.join("player");
        t.take_messages(player);

        t.chat(player, "/sp c");

        assert_eq!(t.take_chats(player), ["Unknown command /sp"]);
    }

    #[test]
    fn warmup_pucks_are_centered_and_zero_pucks_is_allowed() {
        for pucks in [0, 1, 3] {
            let mut t = TestServer::new(PermanentWarmup::new(pucks, SpawnPoint::Center));
            t.join("player");
            t.tick();

            let ctx = t.ctx();
            let center_x = ctx.rink.width / 2.0;
            let offsets: Vec<f32> = ctx
                .objects
                .pucks()
                .map(|(_, puck)| puck.body.pos.x - center_x)
                .collect();
            assert_eq!(offsets.len(), pucks);
            assert!(offsets.iter().sum::<f32>().abs() < 1e-4, "{offsets:?}");
        }
    }

    #[test]
    fn warmup_mode_stays_in_warmup() {
        let mut t = TestServer::new(PermanentWarmup::new(3, SpawnPoint::Center));
        let red = t.join("red");
        let blue = t.join("blue");
        t.press_join(red, Team::Red);
        t.press_join(blue, Team::Blue);
        for _ in 0..5000 {
            let scoreboard = t.tick().unwrap();
            assert_eq!(scoreboard.period, 0);
        }
        assert_eq!(t.ctx().objects.pucks().count(), 3);
    }
}

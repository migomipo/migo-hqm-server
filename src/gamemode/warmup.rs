use crate::game::ScoreboardValues;
use crate::game::{PhysicsEvent, PlayerId};
use crate::gamemode::util::{SpawnPoint, TeamSelection, get_spawnpoint, spawn_warmup_pucks};
use crate::gamemode::{CommandResult, ExitReason, GameContext, GameMode};

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
        ctx: &mut GameContext,
        cmd: &str,
        arg: &str,
        player_id: PlayerId,
    ) -> CommandResult {
        match cmd {
            "fs" => self
                .teams
                .handle_force_spectate_command(ctx, player_id, arg),
            _ => return CommandResult::Unknown,
        }
        CommandResult::Handled
    }

    fn game_started(&mut self, ctx: &mut GameContext) {
        spawn_warmup_pucks(ctx, self.pucks, 1.5);
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

    #[test]
    fn fs_forces_a_player_off_the_ice() {
        let mut t = TestServer::new(PermanentWarmup::new(1, SpawnPoint::Center));
        let admin = t.join_admin("admin");
        let player = t.join("player");
        t.press_join(player, Team::Red);
        t.tick();
        assert!(t.ctx().objects.skater(player).is_some());
        t.take_messages(admin);

        t.chat(admin, &format!("/fs {}", player.index));

        assert!(t.ctx().objects.skater(player).is_none());
        assert_eq!(t.take_chats(admin), ["player forced off ice by admin"]);
    }
}

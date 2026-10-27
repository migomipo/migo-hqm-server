use crate::game::{PhysicsEvent, PlayerId, RulesState, ScoreboardValues};
use crate::gamemode::util::{
    GameProgress, SpawnPoint, TeamSelection, get_spawnpoint, spawn_warmup_pucks,
};
use crate::gamemode::{CommandResult, ExitReason, GameContext, GameMode};

mod commands;
mod game;
#[cfg(test)]
pub(crate) mod test_game;

pub use game::{
    ALLOWED_POSITIONS, FACEOFF_COMMAND_DELAY, IcingConfiguration, Match, MatchConfiguration,
    MatchEvent, MatchSettings, OffsideConfiguration, OffsideLineConfiguration,
    REPLAY_HISTORY_LENGTH, TwoLinePassConfiguration,
};

// There is only one per server, so a match is kept inline even though it is much larger than
// warmup.
#[allow(clippy::large_enum_variant)]
pub enum StandardMatchState {
    WaitingForGame(WarmupState),
    Game(Match),
}
pub struct WarmupState {
    pub(crate) time: u32,
    pub(crate) score: (u32, u32),
    /// Whether an admin has paused the game. The game does not start while paused.
    paused: bool,
}
impl WarmupState {
    /// Ticks of warmup once both teams have players, before the game starts.
    const TIME: u32 = 2000;

    fn new() -> Self {
        Self {
            time: Self::TIME,
            score: (0, 0),
            paused: false,
        }
    }
    fn before_tick(&mut self, red_players: usize, blue_players: usize) -> Option<(u32, u32)> {
        if red_players == 0 || blue_players == 0 {
            self.time = Self::TIME;
            return None;
        }
        if self.paused {
            return None;
        }
        self.time = self.time.saturating_sub(1);
        (self.time == 0).then_some(self.score)
    }
    fn scoreboard(&self) -> ScoreboardValues {
        ScoreboardValues {
            rules_state: RulesState::default(),
            red_score: self.score.0,
            blue_score: self.score.1,
            period: 0,
            time: self.time,
            goal_message_timer: 0,
            game_over: false,
        }
    }
}
pub struct StandardMatchGameMode {
    pub settings: MatchSettings,
    pub state: StandardMatchState,
    pub spawn_point: SpawnPoint,
    pub teams: TeamSelection,
}
impl StandardMatchGameMode {
    pub fn new(config: MatchConfiguration, team_max: usize, spawn_point: SpawnPoint) -> Self {
        Self {
            settings: MatchSettings::new(config),
            state: StandardMatchState::WaitingForGame(WarmupState::new()),
            spawn_point,
            teams: TeamSelection::new(team_max),
        }
    }
    fn update_players(&mut self, ctx: &mut GameContext) -> (usize, usize) {
        let point = self.spawn_point;
        self.teams
            .update(ctx, |rink, team, _| get_spawnpoint(rink, team, point))
    }
    /// Whether a game is on and play has not been stopped.
    pub(crate) fn is_playing(&self) -> bool {
        matches!(&self.state, StandardMatchState::Game(game) if game.is_playing())
    }
    fn start_game(&mut self, preliminary_score: (u32, u32)) {
        self.state =
            StandardMatchState::Game(Match::start(&self.settings.config, preliminary_score));
    }
}
impl GameMode for StandardMatchGameMode {
    fn init(&mut self, ctx: &mut GameContext) {
        ctx.replay
            .set_history_length(REPLAY_HISTORY_LENGTH as usize);
    }
    fn before_tick(&mut self, ctx: &mut GameContext) {
        let (red, blue) = self.update_players(ctx);
        match &mut self.state {
            StandardMatchState::WaitingForGame(warmup) => {
                if let Some(score) = warmup.before_tick(red, blue) {
                    self.start_game(score);
                }
            }
            StandardMatchState::Game(game) => {
                if game.before_tick(ctx, &self.settings) == GameProgress::Finished {
                    ctx.new_game();
                }
            }
        }
    }
    fn after_tick(&mut self, ctx: &mut GameContext, events: &[PhysicsEvent]) -> ScoreboardValues {
        if let StandardMatchState::Game(game) = &mut self.state {
            // The match announces its events in chat, which is all the standard mode does with
            // them.
            let _ = game.after_tick(ctx, &self.settings, events);
        }
        match &self.state {
            StandardMatchState::WaitingForGame(warmup) => warmup.scoreboard(),
            StandardMatchState::Game(game) => game.scoreboard(),
        }
    }
    fn handle_command(
        &mut self,
        ctx: &mut GameContext,
        command: &str,
        arg: &str,
        player: PlayerId,
    ) -> CommandResult {
        match command {
            "set" => self.handle_set(ctx, arg, player),
            "rules" => self.handle_rules(ctx, player),
            "start" => self.handle_admin_command(ctx, player, "Game started", Self::start),
            "faceoff" => {
                self.handle_admin_command(ctx, player, "Faceoff initiated", Self::call_faceoff)
            }
            "pause" => self.handle_admin_command(ctx, player, "Game paused", Self::pause),
            "unpause" | "resume" => {
                self.handle_admin_command(ctx, player, "Game resumed", Self::unpause)
            }
            "sp" | "setposition" => {
                if let Some(position) = self.settings.set_preferred_position(player, arg) {
                    ctx.messages.add_directed_server_chat_message(
                        format!("Preferred faceoff position set to {position}"),
                        player,
                    );
                } else {
                    ctx.messages
                        .add_directed_server_chat_message("Invalid faceoff position", player);
                }
            }
            "fs" => self.teams.handle_force_spectate_command(ctx, player, arg),
            _ => return CommandResult::Unknown,
        }
        CommandResult::Handled
    }
    fn game_started(&mut self, ctx: &mut GameContext) {
        self.state = StandardMatchState::WaitingForGame(WarmupState::new());
        self.settings.forget_departed_players(ctx);
        let config = &self.settings.config;
        spawn_warmup_pucks(ctx, config.warmup_pucks, config.spawn_puck_altitude);
    }
    fn before_player_exit(
        &mut self,
        _ctx: &mut GameContext,
        player: PlayerId,
        _reason: ExitReason,
    ) {
        self.teams.remove_player(player);
        self.settings.remove_player(player);
    }
    fn server_list_team_size(&self) -> u32 {
        self.teams.team_max as u32
    }
    fn include_tick_in_recording(&self) -> bool {
        matches!(self.state, StandardMatchState::Game(_))
    }
    /// Players can change hands in warmup and whenever play has stopped, but not during play.
    fn hand_change_allowed(&self) -> bool {
        !self.is_playing()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::Puck;
    use crate::server::test_server::TestServer;
    use glam::Vec3;
    use glamx::Rot3;
    use test_game::MatchGame;

    #[test]
    fn finished_game_is_replaced_by_a_new_one_in_the_same_tick() {
        let config = MatchConfiguration {
            warmup_pucks: 3,
            ..MatchConfiguration::default()
        };
        let mut t = TestServer::new(StandardMatchGameMode::new(config, 5, SpawnPoint::Center));
        t.join("player");
        t.tick();
        t.mode.state = StandardMatchState::Game(Match::in_game_over_break(&config, (2, 1), 1));
        let mut ctx = t.ctx();
        ctx.objects.remove_all_pucks();
        ctx.objects
            .spawn_puck(Puck::new(Vec3::ZERO, Rot3::IDENTITY));

        t.tick();

        // The old game's puck is gone, and game_started has reset the game mode and put the
        // warmup pucks on the ice.
        assert_eq!(t.ctx().objects.pucks().count(), 3);
        assert!(matches!(
            t.mode.state,
            StandardMatchState::WaitingForGame(WarmupState {
                time: WarmupState::TIME,
                score: (0, 0),
                paused: false,
            })
        ));
    }

    #[test]
    fn match_commands_are_handled_and_others_are_unknown() {
        let mode = StandardMatchGameMode::new(MatchConfiguration::default(), 5, SpawnPoint::Center);
        let mut t = TestServer::new(mode);
        let player = t.join("player");
        t.take_messages(player);

        t.chat(player, "/sp lw");
        t.chat(player, "/sp nowhere");
        t.chat(player, "/nosuchcommand");

        assert_eq!(
            t.take_chats(player),
            [
                "Preferred faceoff position set to LW",
                "Invalid faceoff position",
                "Unknown command /nosuchcommand"
            ]
        );
    }

    #[test]
    fn team_max_limits_the_players_on_each_team() {
        let mode = StandardMatchGameMode::new(MatchGame::quick_config(), 1, SpawnPoint::Center);
        let mut game = MatchGame::join_with(mode, 2, 0);
        game.ticks(5);
        assert_eq!(game.t.ctx().players.count_team_members(), (1, 0));
    }

    #[test]
    fn spawn_center_puts_joining_players_at_the_center_circle() {
        let mut game = MatchGame::join(MatchGame::quick_config(), 1, 0);
        game.ticks(2);
        let pos = game.skater_pos(game.red[0]);
        let center = Vec3::new(15.0, pos.y, 30.5);
        assert!(pos.distance(center) < 4.5, "{pos}");
    }

    #[test]
    fn spawn_bench_puts_joining_players_next_to_the_bench() {
        let mode = StandardMatchGameMode::new(MatchGame::quick_config(), 5, SpawnPoint::Bench);
        let mut game = MatchGame::join_with(mode, 1, 0);
        game.ticks(2);
        let pos = game.skater_pos(game.red[0]);
        assert!(pos.x < 2.0, "{pos}");
    }

    #[test]
    fn warmup_pucks_sets_the_number_of_pucks_in_warmup() {
        let config = MatchConfiguration {
            warmup_pucks: 4,
            ..MatchGame::quick_config()
        };
        let mut game = MatchGame::join(config, 1, 0);
        game.ticks(2);
        assert_eq!(game.scoreboard.period, 0);
        assert_eq!(game.pucks().len(), 4);
    }
}

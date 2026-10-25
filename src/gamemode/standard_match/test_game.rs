//! A match on a [TestServer] for scenario tests: players on both teams, and helpers for placing
//! skaters, moving the puck and listening to chat.

use glam::Vec3;
use glamx::Rot3;

use crate::game::{HALF_TURN_Y, PhysicsEvent, PlayerId, ScoreboardValues, Team};
use crate::gamemode::util::SpawnPoint;
use crate::gamemode::{CommandResult, ExitReason, GameContext, GameMode};
use crate::players::PlayerListExt;
use crate::server::test_server::TestServer;

use super::{MatchConfiguration, StandardMatchGameMode};

/// Runs a game mode and keeps the physics events of the last simulated tick.
pub(crate) struct Recorder<M> {
    pub(crate) mode: M,
    events: Vec<PhysicsEvent>,
}

impl<M: GameMode> GameMode for Recorder<M> {
    fn init(&mut self, ctx: &mut GameContext) {
        self.mode.init(ctx)
    }
    fn before_tick(&mut self, ctx: &mut GameContext) {
        self.mode.before_tick(ctx)
    }
    fn after_tick(&mut self, ctx: &mut GameContext, events: &[PhysicsEvent]) -> ScoreboardValues {
        self.events = events.to_vec();
        self.mode.after_tick(ctx, events)
    }
    fn handle_command(
        &mut self,
        ctx: &mut GameContext,
        cmd: &str,
        arg: &str,
        player_id: PlayerId,
    ) -> CommandResult {
        self.mode.handle_command(ctx, cmd, arg, player_id)
    }
    fn game_started(&mut self, ctx: &mut GameContext) {
        self.mode.game_started(ctx)
    }
    fn game_abandoned(&mut self, ctx: &mut GameContext) {
        self.mode.game_abandoned(ctx)
    }
    fn before_player_exit(
        &mut self,
        ctx: &mut GameContext,
        player_id: PlayerId,
        reason: ExitReason,
    ) {
        self.mode.before_player_exit(ctx, player_id, reason)
    }
    fn after_player_join(&mut self, ctx: &mut GameContext, player_id: PlayerId) {
        self.mode.after_player_join(ctx, player_id)
    }
    fn server_list_team_size(&self) -> u32 {
        self.mode.server_list_team_size()
    }
    fn include_tick_in_recording(&self) -> bool {
        self.mode.include_tick_in_recording()
    }
    fn hand_change_allowed(&self) -> bool {
        self.mode.hand_change_allowed()
    }
}

/// Places on the ice, as z coordinates, for a team attacking towards low z like Red.
pub(crate) mod red_attack {
    /// In Red's own zone, behind its own blue line.
    pub const OWN_ZONE: f32 = 45.0;
    /// Between Red's own blue line and the center line.
    pub const OWN_HALF: f32 = 34.0;
    /// Past the center line, but not yet in the offensive zone.
    pub const PAST_CENTER: f32 = 27.0;
    /// In Red's offensive zone, past the offensive blue line.
    pub const IN_ZONE: f32 = 15.0;
}

/// The x coordinate pucks are passed along, clear of the nets.
pub(crate) const PUCK_LANE: f32 = 20.0;

/// The x coordinate receivers wait at, away from the puck lane.
pub(crate) const RECEIVER_LANE: f32 = 10.0;

/// A match with players on both teams, played on a [TestServer].
///
/// Red attacks towards the blue end at low z, and Blue towards the red end at high z. The first
/// red player is an administrator and receives the chat that tests check.
pub(crate) struct MatchGame {
    pub(crate) t: TestServer<Recorder<StandardMatchGameMode>>,
    pub(crate) red: Vec<PlayerId>,
    pub(crate) blue: Vec<PlayerId>,
    /// The scoreboard of the last tick.
    pub(crate) scoreboard: ScoreboardValues,
    /// Chat messages the first red player has received.
    pub(crate) heard: Vec<String>,
}

impl MatchGame {
    /// Short breaks, so that tests get to play quickly.
    pub(crate) fn quick_config() -> MatchConfiguration {
        MatchConfiguration {
            time_break: 1,
            time_intermission: 1,
            ..MatchConfiguration::default()
        }
    }

    /// Joins players to both teams, without playing any ticks yet.
    pub(crate) fn join(config: MatchConfiguration, red: usize, blue: usize) -> Self {
        Self::join_with(
            StandardMatchGameMode::new(config, 5, SpawnPoint::Center),
            red,
            blue,
        )
    }

    pub(crate) fn join_with(mode: StandardMatchGameMode, red: usize, blue: usize) -> Self {
        let mut t = TestServer::new(Recorder {
            mode,
            events: vec![],
        });
        let red: Vec<_> = (0..red).map(|i| t.join(&format!("red{i}"))).collect();
        let blue: Vec<_> = (0..blue).map(|i| t.join(&format!("blue{i}"))).collect();
        if let Some(&admin) = red.first() {
            t.server
                .state
                .player_message_state
                .players
                .get_player_mut(admin)
                .unwrap()
                .is_admin = true;
        }
        for &player in &red {
            t.press_join(player, Team::Red);
        }
        for &player in &blue {
            t.press_join(player, Team::Blue);
        }
        let mut game = MatchGame {
            t,
            red,
            blue,
            scoreboard: ScoreboardValues::default(),
            heard: vec![],
        };
        game.hear();
        game.heard.clear();
        game
    }

    /// Joins players to both teams, plays until the opening faceoff has been made, and waits for
    /// the skaters to land on the ice.
    pub(crate) fn start(config: MatchConfiguration, red: usize, blue: usize) -> Self {
        let mut game = Self::join(config, red, blue);
        game.play_until_faceoff();
        game.land();
        game
    }

    /// Ticks until every skater stands on the ice. Skaters appear above the ice at faceoffs.
    pub(crate) fn land(&mut self) {
        self.tick_until(1000, |game| {
            game.server_objects().all(|object| match object {
                crate::server::GameObject::Skater(_, skater) => {
                    skater.body.pos.y < 1.0 && skater.body.linear_velocity.y.abs() < 0.001
                }
                crate::server::GameObject::Puck(..) => true,
            })
        });
        self.heard.clear();
    }

    /// Plays through warmup and the opening break until the opening faceoff has been made.
    pub(crate) fn play_until_faceoff(&mut self) {
        self.tick_until(10_000, |game| game.is_playing());
        self.heard.clear();
    }

    pub(crate) fn admin(&self) -> PlayerId {
        self.red[0]
    }

    /// Whether play is on: a game is running and play has not been stopped.
    ///
    /// This asks the game mode, as the scoreboard looks the same during play and after an admin
    /// has stopped it.
    pub(crate) fn is_playing(&self) -> bool {
        self.t.mode.mode.is_playing()
    }

    pub(crate) fn tick(&mut self) {
        if let Some(scoreboard) = self.t.tick() {
            self.scoreboard = scoreboard;
        }
        self.hear();
    }

    pub(crate) fn ticks(&mut self, ticks: u32) {
        for _ in 0..ticks {
            self.tick();
        }
    }

    /// Ticks until `done` returns true, and returns how many ticks that took. Panics if that
    /// takes more than `max_ticks`.
    pub(crate) fn tick_until(
        &mut self,
        max_ticks: u32,
        mut done: impl FnMut(&Self) -> bool,
    ) -> u32 {
        for ticks in 0..max_ticks {
            if done(self) {
                return ticks;
            }
            self.tick();
        }
        assert!(done(self), "condition not reached within {max_ticks} ticks");
        max_ticks
    }

    pub(crate) fn hear(&mut self) {
        let chats = self.t.take_chats(self.red[0]);
        self.heard.extend(chats);
    }

    pub(crate) fn heard(&self, text: &str) -> bool {
        self.heard.iter().any(|chat| chat == text)
    }

    /// Ticks until the first red player has heard `text`, for at most `max_ticks`.
    pub(crate) fn wait_to_hear(&mut self, max_ticks: u32, text: &str) {
        let text = text.to_owned();
        self.tick_until(max_ticks, |game| game.heard(&text));
    }

    pub(crate) fn chat(&mut self, player: PlayerId, message: &str) {
        self.t.chat(player, message);
        self.hear();
    }

    pub(crate) fn rotation(team: Team) -> Rot3 {
        match team {
            Team::Red => Rot3::IDENTITY,
            Team::Blue => HALF_TURN_Y,
        }
    }

    pub(crate) fn team(&self, player: PlayerId) -> Team {
        if self.red.contains(&player) {
            Team::Red
        } else {
            Team::Blue
        }
    }

    /// Moves a skater to `(x, z)` on the ice, facing the end its team attacks. The skater keeps
    /// its height, so that a skater standing on the ice stays on it.
    pub(crate) fn place(&mut self, player: PlayerId, x: f32, z: f32) {
        let rot = Self::rotation(self.team(player));
        let mut ctx = self.t.ctx();
        let skater = ctx
            .objects
            .skater_mut(player)
            .expect("player is on the ice");
        let y = skater.body.pos.y;
        skater.teleport(Vec3::new(x, y, z), rot);
    }

    /// Moves every skater out of the way, to the boards at the red end.
    pub(crate) fn clear_ice(&mut self) {
        let players: Vec<_> = self.red.iter().chain(&self.blue).copied().collect();
        for (i, player) in players.into_iter().enumerate() {
            self.place(player, 1.5, 50.0 + 1.5 * i as f32);
        }
    }

    pub(crate) fn skater_pos(&self, player: PlayerId) -> Vec3 {
        self.server_objects()
            .find_map(|object| match object {
                crate::server::GameObject::Skater(id, skater) if *id == player => {
                    Some(skater.body.pos)
                }
                _ => None,
            })
            .expect("player is on the ice")
    }

    pub(crate) fn server_objects(&self) -> impl Iterator<Item = &crate::server::GameObject> {
        self.t.server.state.objects.iter().flatten()
    }

    pub(crate) fn pucks(&self) -> Vec<Vec3> {
        self.server_objects()
            .filter_map(|object| match object {
                crate::server::GameObject::Puck(_, puck) => Some(puck.body.pos),
                _ => None,
            })
            .collect()
    }

    /// Moves the puck and sets its velocity, in meters per tick.
    pub(crate) fn set_puck(&mut self, pos: Vec3, velocity: Vec3) {
        let mut ctx = self.t.ctx();
        let (_, puck) = ctx
            .objects
            .pucks_mut()
            .next()
            .expect("a puck is on the ice");
        puck.body.pos = pos;
        puck.body.linear_velocity = velocity;
        puck.body.angular_velocity = Vec3::ZERO;
    }

    pub(crate) fn touched(&self, player: PlayerId) -> bool {
        self.t
            .mode
            .events
            .iter()
            .any(|event| matches!(event, PhysicsEvent::PuckTouch { player: p, .. } if *p == player))
    }

    /// Puts the puck against a player's stick blade until the physics registers the touch.
    ///
    /// A touch needs the edge of the puck inside the blade with its center outside, so the puck
    /// is put next to the blade, on alternating sides.
    pub(crate) fn touch(&mut self, player: PlayerId) {
        for attempt in 0..20 {
            let skater = self.t.ctx().objects.skater(player).unwrap().clone();
            let side = if attempt % 2 == 0 { 1.0 } else { -1.0 };
            let pos = skater.stick_pos + skater.stick_rot * Vec3::new(side * 0.12, 0.0, 0.0);
            self.set_puck(pos, Vec3::ZERO);
            self.tick();
            if self.touched(player) {
                return;
            }
        }
        panic!("{player} could not touch the puck");
    }

    /// Lets a player touch the puck at `(x, z)` and send it along z with `speed` meters per tick
    /// towards the end their team attacks.
    pub(crate) fn pass_from(&mut self, player: PlayerId, x: f32, z: f32, speed: f32) {
        self.place(player, x, z);
        self.touch(player);
        let puck = self.pucks()[0];
        // Out of the way, so that the stick does not touch the puck again.
        self.place(player, x - 6.0, z);
        let direction = match self.team(player) {
            Team::Red => -1.0,
            Team::Blue => 1.0,
        };
        self.set_puck(puck, Vec3::new(0.0, 0.0, direction * speed));
    }

    /// Ticks until the puck has passed `z` in the direction it moves, for at most 300 ticks.
    pub(crate) fn wait_for_puck_past(&mut self, z: f32) {
        let start = self.pucks()[0].z;
        self.tick_until(300, |game| {
            let puck_z = game.pucks().first().map_or(start, |puck| puck.z);
            if start > z { puck_z < z } else { puck_z > z }
        });
    }

    /// Scores a goal for the player's team with a shot into the other team's net.
    pub(crate) fn score(&mut self, player: PlayerId) {
        self.clear_ice();
        let z = match self.team(player) {
            Team::Red => 12.0,
            Team::Blue => 49.0,
        };
        let goals_before = self.scoreboard.red_score + self.scoreboard.blue_score;
        self.pass_from(player, 15.0, z, 0.3);
        self.tick_until(200, |game| {
            game.scoreboard.red_score + game.scoreboard.blue_score > goals_before
        });
    }

    /// Plays until play is on again after a stoppage, and the skaters stand on the ice.
    pub(crate) fn wait_for_play(&mut self) {
        self.tick_until(10_000, |game| game.is_playing());
        self.land();
    }
}

/// Red ices the puck from its own zone, past Blue's goal line.
pub(crate) fn ice_the_puck(game: &mut MatchGame) {
    game.clear_ice();
    game.pass_from(game.red[0], PUCK_LANE, red_attack::OWN_ZONE, 0.4);
    game.wait_for_puck_past(3.0);
}

/// A server in match mode with players that are not on the ice.
pub(crate) fn match_server() -> TestServer<StandardMatchGameMode> {
    TestServer::new(StandardMatchGameMode::new(
        MatchGame::quick_config(),
        5,
        SpawnPoint::Center,
    ))
}

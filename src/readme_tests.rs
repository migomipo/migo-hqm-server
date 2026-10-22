//! Scenario tests for the behaviour that README.md documents.
//!
//! Each test plays out one documented setting, rule or command on a [TestServer] and checks what
//! players would see: chat messages, the scoreboard, and where skaters and pucks are. Documented
//! behaviour that the server does not have is tested too, with the test marked `#[ignore]` and
//! the reason given, so `cargo test -- --ignored` lists what the README promises but the server
//! does not do.

use glam::Vec3;
use glamx::Rot3;

use crate::ReplayRecording;
use crate::game::{PhysicsEvent, PlayerId, RulesState, ScoreboardValues, SkaterHand, Team};
use crate::gamemode::shootout::{ShootoutGameConfiguration, ShootoutGameMode};
use crate::gamemode::standard_match::{
    IcingConfiguration, MatchConfiguration, OffsideConfiguration, OffsideLineConfiguration,
    StandardMatchGameMode, TwoLinePassConfiguration,
};
use crate::gamemode::util::SpawnPoint;
use crate::gamemode::warmup::PermanentWarmup;
use crate::gamemode::{CommandResult, ExitReason, GameContext, GameMode};
use crate::players::{PlayerListExt, ServerPlayerData};
use crate::server::test_server::TestServer;

/// Runs a game mode and keeps the physics events of the last simulated tick.
struct Recorder<M> {
    mode: M,
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
}

/// Places on the ice, as z coordinates, for a team attacking towards low z like Red.
mod red_attack {
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
const PUCK_LANE: f32 = 20.0;
/// The x coordinate receivers wait at, away from the puck lane.
const RECEIVER_LANE: f32 = 10.0;

/// A match with players on both teams, played on a [TestServer].
///
/// Red attacks towards the blue end at low z, and Blue towards the red end at high z. The first
/// red player is an administrator and receives the chat that tests check.
struct MatchGame {
    t: TestServer<Recorder<StandardMatchGameMode>>,
    red: Vec<PlayerId>,
    blue: Vec<PlayerId>,
    /// The scoreboard of the last tick.
    scoreboard: ScoreboardValues,
    /// Chat messages the first red player has received.
    heard: Vec<String>,
}

impl MatchGame {
    /// Short breaks, so that tests get to play quickly.
    fn quick_config() -> MatchConfiguration {
        MatchConfiguration {
            time_break: 1,
            time_intermission: 1,
            ..MatchConfiguration::default()
        }
    }

    /// Joins players to both teams, without playing any ticks yet.
    fn join(config: MatchConfiguration, red: usize, blue: usize) -> Self {
        Self::join_with(
            StandardMatchGameMode::new(config, 5, SpawnPoint::Center),
            red,
            blue,
        )
    }

    fn join_with(mode: StandardMatchGameMode, red: usize, blue: usize) -> Self {
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
    fn start(config: MatchConfiguration, red: usize, blue: usize) -> Self {
        let mut game = Self::join(config, red, blue);
        game.play_until_faceoff();
        game.land();
        game
    }

    /// Ticks until every skater stands on the ice. Skaters appear above the ice at faceoffs.
    fn land(&mut self) {
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
    fn play_until_faceoff(&mut self) {
        self.tick_until(10_000, |game| game.is_playing());
        self.heard.clear();
    }

    fn admin(&self) -> PlayerId {
        self.red[0]
    }

    /// Whether play is on: the clock runs in a period and play has not been stopped.
    fn is_playing(&self) -> bool {
        self.scoreboard.period >= 1
            && self.scoreboard.time > 0
            && self.scoreboard.goal_message_timer == 0
            && matches!(self.scoreboard.rules_state, RulesState::Regular { .. })
    }

    fn tick(&mut self) {
        if let Some(scoreboard) = self.t.tick() {
            self.scoreboard = scoreboard;
        }
        self.hear();
    }

    fn ticks(&mut self, ticks: u32) {
        for _ in 0..ticks {
            self.tick();
        }
    }

    /// Ticks until `done` returns true, and returns how many ticks that took. Panics if that
    /// takes more than `max_ticks`.
    fn tick_until(&mut self, max_ticks: u32, mut done: impl FnMut(&Self) -> bool) -> u32 {
        for ticks in 0..max_ticks {
            if done(self) {
                return ticks;
            }
            self.tick();
        }
        assert!(done(self), "condition not reached within {max_ticks} ticks");
        max_ticks
    }

    fn hear(&mut self) {
        let chats = self.t.take_chats(self.red[0]);
        self.heard.extend(chats);
    }

    fn heard(&self, text: &str) -> bool {
        self.heard.iter().any(|chat| chat == text)
    }

    /// Ticks until the first red player has heard `text`, for at most `max_ticks`.
    fn wait_to_hear(&mut self, max_ticks: u32, text: &str) {
        let text = text.to_owned();
        self.tick_until(max_ticks, |game| game.heard(&text));
    }

    fn chat(&mut self, player: PlayerId, message: &str) {
        self.t.chat(player, message);
        self.hear();
    }

    fn rotation(team: Team) -> Rot3 {
        match team {
            Team::Red => Rot3::IDENTITY,
            Team::Blue => Rot3::from_rotation_y(std::f32::consts::PI),
        }
    }

    fn team(&self, player: PlayerId) -> Team {
        if self.red.contains(&player) {
            Team::Red
        } else {
            Team::Blue
        }
    }

    /// Moves a skater to `(x, z)` on the ice, facing the end its team attacks. The skater keeps
    /// its height, so that a skater standing on the ice stays on it.
    fn place(&mut self, player: PlayerId, x: f32, z: f32) {
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
    fn clear_ice(&mut self) {
        let players: Vec<_> = self.red.iter().chain(&self.blue).copied().collect();
        for (i, player) in players.into_iter().enumerate() {
            self.place(player, 1.5, 50.0 + 1.5 * i as f32);
        }
    }

    fn skater_pos(&self, player: PlayerId) -> Vec3 {
        self.server_objects()
            .find_map(|object| match object {
                crate::server::GameObject::Skater(id, skater) if *id == player => {
                    Some(skater.body.pos)
                }
                _ => None,
            })
            .expect("player is on the ice")
    }

    fn server_objects(&self) -> impl Iterator<Item = &crate::server::GameObject> {
        self.t.server.state.objects.iter().flatten()
    }

    fn pucks(&self) -> Vec<Vec3> {
        self.server_objects()
            .filter_map(|object| match object {
                crate::server::GameObject::Puck(_, puck) => Some(puck.body.pos),
                _ => None,
            })
            .collect()
    }

    /// Moves the puck and sets its velocity, in meters per tick.
    fn set_puck(&mut self, pos: Vec3, velocity: Vec3) {
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

    fn touched(&self, player: PlayerId) -> bool {
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
    fn touch(&mut self, player: PlayerId) {
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
    fn pass_from(&mut self, player: PlayerId, x: f32, z: f32, speed: f32) {
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
    fn wait_for_puck_past(&mut self, z: f32) {
        let start = self.pucks()[0].z;
        self.tick_until(300, |game| {
            let puck_z = game.pucks().first().map_or(start, |puck| puck.z);
            if start > z { puck_z < z } else { puck_z > z }
        });
    }

    /// Scores a goal for the player's team with a shot into the other team's net.
    fn score(&mut self, player: PlayerId) {
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
    fn wait_for_play(&mut self) {
        self.tick_until(10_000, |game| game.is_playing());
        self.land();
    }
}

// ----- Server settings -----

#[test]
fn welcome_messages_are_sent_to_joining_players() {
    let mut t = TestServer::new(StandardMatchGameMode::new(
        MatchConfiguration::default(),
        5,
        SpawnPoint::Center,
    ));
    t.server.config.welcome = vec!["Welcome!".to_owned(), "Have fun".to_owned()];
    let player = t.join("player");
    let chats = t.take_chats(player);
    assert_eq!(chats[..2], ["Welcome!", "Have fun"]);
}

#[test]
fn player_max_limits_the_players_on_the_server() {
    let mut t = TestServer::new(PermanentWarmup::new(1, SpawnPoint::Center));
    t.server.config.player_max = 2;
    t.join("first");
    t.join("second");
    assert!(t.try_join("third").is_none());
}

#[test]
fn team_max_limits_the_players_on_each_team() {
    let mode = StandardMatchGameMode::new(MatchGame::quick_config(), 1, SpawnPoint::Center);
    let mut game = MatchGame::join_with(mode, 2, 0);
    game.ticks(5);
    assert_eq!(game.t.ctx().players.count_team_members(), (1, 0));
}

// ----- Game settings -----

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
fn spawn_offset_sets_the_centers_distance_from_the_faceoff_spot() {
    for offset in [1.0, 4.0] {
        let config = MatchConfiguration {
            spawn_point_offset: offset,
            ..MatchGame::quick_config()
        };
        let game = MatchGame::start(config, 1, 1);
        let center_z = game.t.server.rink.center_line.z;
        let red = game.skater_pos(game.red[0]);
        let blue = game.skater_pos(game.blue[0]);
        assert!(((red.z - center_z) - offset).abs() < 0.1, "{red}");
        assert!(((center_z - blue.z) - offset).abs() < 0.1, "{blue}");
    }
}

#[test]
fn spawn_altitudes_set_where_players_and_the_puck_appear_at_faceoffs() {
    let config = MatchConfiguration {
        spawn_player_altitude: 4.0,
        spawn_puck_altitude: 3.0,
        ..MatchGame::quick_config()
    };
    let mut game = MatchGame::join(config, 1, 1);
    game.play_until_faceoff();
    // One tick of physics has passed since the faceoff, so allow for a little falling.
    let skater = game.skater_pos(game.red[0]);
    assert!((skater.y - 4.0).abs() < 0.1, "{skater}");
    let puck = game.pucks()[0];
    assert!((puck.y - 3.0).abs() < 0.1, "{puck}");
}

#[test]
fn icing_off_does_not_call_icing() {
    let mut game = MatchGame::start(MatchGame::quick_config(), 1, 1);
    game.clear_ice();
    game.pass_from(game.red[0], PUCK_LANE, red_attack::OWN_ZONE, 0.4);
    game.wait_for_puck_past(3.0);
    game.ticks(20);
    assert!(
        !game.heard.iter().any(|chat| chat.contains("Icing")),
        "{:?}",
        game.heard
    );
}

/// Red ices the puck from its own zone, past Blue's goal line.
fn ice_the_puck(game: &mut MatchGame) {
    game.clear_ice();
    game.pass_from(game.red[0], PUCK_LANE, red_attack::OWN_ZONE, 0.4);
    game.wait_for_puck_past(3.0);
}

#[test]
fn touch_icing_warns_and_is_called_when_a_defender_touches_the_puck() {
    let config = MatchConfiguration {
        icing: IcingConfiguration::Touch,
        ..MatchGame::quick_config()
    };
    let mut game = MatchGame::start(config, 1, 1);
    ice_the_puck(&mut game);
    assert!(game.heard("Icing warning"), "{:?}", game.heard);
    assert!(matches!(
        game.scoreboard.rules_state,
        RulesState::Regular {
            icing_warning: true,
            ..
        }
    ));

    game.place(game.blue[0], RECEIVER_LANE, 10.0);
    game.touch(game.blue[0]);
    assert!(game.heard("Icing"), "{:?}", game.heard);
    assert_eq!(game.scoreboard.rules_state, RulesState::Icing);
}

#[test]
fn touch_icing_is_waved_off_when_the_icing_team_touches_the_puck_first() {
    let config = MatchConfiguration {
        icing: IcingConfiguration::Touch,
        ..MatchGame::quick_config()
    };
    let mut game = MatchGame::start(config, 1, 1);
    ice_the_puck(&mut game);
    game.place(game.red[0], RECEIVER_LANE, 10.0);
    game.touch(game.red[0]);
    assert!(game.heard("Icing waved off"), "{:?}", game.heard);
    assert!(!game.heard("Icing"));
}

#[test]
fn no_touch_icing_is_called_when_the_puck_crosses_the_goal_line() {
    let config = MatchConfiguration {
        icing: IcingConfiguration::NoTouch,
        ..MatchGame::quick_config()
    };
    let mut game = MatchGame::start(config, 1, 1);
    ice_the_puck(&mut game);
    assert!(game.heard("Icing"), "{:?}", game.heard);
    assert!(!game.heard("Icing warning"));
    assert_eq!(game.scoreboard.rules_state, RulesState::Icing);
}

/// Red passes from the neutral zone into Blue's zone, where a red teammate already is.
fn offside_pass(config: MatchConfiguration) -> MatchGame {
    let mut game = MatchGame::start(config, 2, 1);
    game.clear_ice();
    game.place(game.red[1], RECEIVER_LANE, red_attack::IN_ZONE);
    game.pass_from(game.red[0], PUCK_LANE, red_attack::PAST_CENTER, 0.3);
    game.wait_for_puck_past(red_attack::IN_ZONE + 3.0);
    game
}

#[test]
fn offside_off_does_not_call_offside() {
    let mut game = offside_pass(MatchGame::quick_config());
    game.touch(game.red[1]);
    assert!(
        !game.heard.iter().any(|chat| chat.contains("Offside")),
        "{:?}",
        game.heard
    );
}

#[test]
fn offside_on_warns_and_is_called_when_the_offside_player_touches_the_puck() {
    let config = MatchConfiguration {
        offside: OffsideConfiguration::Delayed,
        ..MatchGame::quick_config()
    };
    let mut game = offside_pass(config);
    assert!(game.heard("Offside warning"), "{:?}", game.heard);
    assert!(matches!(
        game.scoreboard.rules_state,
        RulesState::Regular {
            offside_warning: true,
            ..
        }
    ));

    game.touch(game.red[1]);
    assert!(game.heard("Offside"), "{:?}", game.heard);
    assert_eq!(game.scoreboard.rules_state, RulesState::Offside);
}

#[test]
fn immediate_offside_is_called_when_the_puck_enters_the_zone() {
    let config = MatchConfiguration {
        offside: OffsideConfiguration::Immediate,
        ..MatchGame::quick_config()
    };
    let game = offside_pass(config);
    assert!(game.heard("Offside"), "{:?}", game.heard);
    assert!(!game.heard("Offside warning"));
    assert_eq!(game.scoreboard.rules_state, RulesState::Offside);
}

#[test]
#[ignore = "with the center line as offside line, offside still checks for teammates past the blue line, in the code before 2ec56b3 too"]
fn offside_line_center_checks_offside_at_the_center_line() {
    let config = MatchConfiguration {
        offside: OffsideConfiguration::Delayed,
        offside_line: OffsideLineConfiguration::Center,
        ..MatchGame::quick_config()
    };
    let mut game = MatchGame::start(config, 2, 1);
    game.clear_ice();
    // The teammate is past the center line, but not in the offensive zone.
    game.place(game.red[1], RECEIVER_LANE, red_attack::PAST_CENTER);
    game.pass_from(game.red[0], PUCK_LANE, red_attack::OWN_HALF, 0.3);
    game.wait_for_puck_past(red_attack::PAST_CENTER);
    assert!(game.heard("Offside warning"), "{:?}", game.heard);
}

/// Red passes from `from_z` to a teammate waiting at `receiver_z`, and the teammate touches the
/// puck once it has passed them.
fn two_line_pass(
    twoline_pass: TwoLinePassConfiguration,
    from_z: f32,
    receiver_z: f32,
) -> MatchGame {
    let config = MatchConfiguration {
        twoline_pass,
        ..MatchGame::quick_config()
    };
    let mut game = MatchGame::start(config, 2, 1);
    game.clear_ice();
    game.place(game.red[1], RECEIVER_LANE, receiver_z);
    game.pass_from(game.red[0], PUCK_LANE, from_z, 0.3);
    game.wait_for_puck_past(receiver_z);
    game.touch(game.red[1]);
    game
}

fn two_line_pass_called(game: &MatchGame) -> bool {
    game.heard("Two-line pass")
}

#[test]
fn two_line_pass_off_allows_every_pass() {
    use red_attack::*;
    for (from, receiver) in [
        (OWN_ZONE, PAST_CENTER),
        (OWN_HALF, IN_ZONE),
        (OWN_ZONE, IN_ZONE),
    ] {
        let game = two_line_pass(TwoLinePassConfiguration::Off, from, receiver);
        assert!(
            !two_line_pass_called(&game),
            "{from} to {receiver}: {:?}",
            game.heard
        );
    }
}

#[test]
fn two_line_pass_on_disallows_passes_through_own_blue_and_center_lines() {
    use red_attack::*;
    let game = two_line_pass(TwoLinePassConfiguration::On, OWN_ZONE, PAST_CENTER);
    assert!(game.heard("Two-line pass warning"), "{:?}", game.heard);
    assert!(two_line_pass_called(&game), "{:?}", game.heard);

    // A pass through the center line and the offensive blue line is allowed.
    let game = two_line_pass(TwoLinePassConfiguration::On, OWN_HALF, IN_ZONE);
    assert!(!two_line_pass_called(&game), "{:?}", game.heard);
}

#[test]
fn two_line_pass_forward_disallows_passes_through_center_and_offensive_blue_lines() {
    use red_attack::*;
    let game = two_line_pass(TwoLinePassConfiguration::Forward, OWN_HALF, IN_ZONE);
    assert!(game.heard("Two-line pass warning"), "{:?}", game.heard);
    assert!(two_line_pass_called(&game), "{:?}", game.heard);

    // A pass through the own blue line and the center line is allowed.
    let game = two_line_pass(TwoLinePassConfiguration::Forward, OWN_ZONE, PAST_CENTER);
    assert!(!two_line_pass_called(&game), "{:?}", game.heard);
}

#[test]
fn two_line_pass_both_disallows_both_kinds_of_passes() {
    use red_attack::*;
    for (from, receiver) in [(OWN_ZONE, PAST_CENTER), (OWN_HALF, IN_ZONE)] {
        let game = two_line_pass(TwoLinePassConfiguration::Double, from, receiver);
        assert!(
            two_line_pass_called(&game),
            "{from} to {receiver}: {:?}",
            game.heard
        );
    }
}

#[test]
fn two_line_pass_three_disallows_passes_through_all_three_lines() {
    use red_attack::*;
    let game = two_line_pass(TwoLinePassConfiguration::ThreeLine, OWN_ZONE, IN_ZONE);
    assert!(two_line_pass_called(&game), "{:?}", game.heard);

    // Passes through two lines are allowed.
    for (from, receiver) in [(OWN_ZONE, PAST_CENTER), (OWN_HALF, IN_ZONE)] {
        let game = two_line_pass(TwoLinePassConfiguration::ThreeLine, from, receiver);
        assert!(
            !two_line_pass_called(&game),
            "{from} to {receiver}: {:?}",
            game.heard
        );
    }
}

#[test]
fn time_period_sets_the_period_length() {
    let config = MatchConfiguration {
        time_period: 3,
        ..MatchGame::quick_config()
    };
    let mut game = MatchGame::join(config, 1, 1);
    game.play_until_faceoff();
    assert_eq!(game.scoreboard.time, 300);
    let ticks = game.tick_until(1000, |game| game.scoreboard.period == 2);
    assert_eq!(ticks, 300);
}

#[test]
fn time_intermission_sets_the_intermission_length() {
    let config = MatchConfiguration {
        time_period: 1,
        time_intermission: 3,
        ..MatchGame::quick_config()
    };
    let mut game = MatchGame::start(config, 1, 1);
    game.tick_until(1000, |game| game.scoreboard.period == 2);
    let ticks = game.tick_until(1000, |game| game.scoreboard.time > 0);
    assert_eq!(ticks, 300);
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

#[test]
fn periods_sets_how_many_periods_a_game_has() {
    let config = MatchConfiguration {
        periods: 1,
        time_period: 5,
        ..MatchGame::quick_config()
    };
    let mut game = MatchGame::start(config, 1, 1);
    game.score(game.red[0]);
    game.tick_until(1000, |game| game.scoreboard.period == 2);
    assert!(game.scoreboard.game_over);
}

#[test]
fn a_tied_game_goes_to_overtime() {
    let config = MatchConfiguration {
        periods: 1,
        time_period: 1,
        ..MatchGame::quick_config()
    };
    let mut game = MatchGame::start(config, 1, 1);
    game.tick_until(1000, |game| game.scoreboard.period == 2);
    assert!(!game.scoreboard.game_over);
    game.tick_until(1000, |game| game.scoreboard.time > 0);
    assert_eq!(game.scoreboard.period, 2);
}

#[test]
fn mercy_ends_the_game_when_a_team_leads_by_enough_goals() {
    let config = MatchConfiguration {
        mercy: 2,
        ..MatchGame::quick_config()
    };
    let mut game = MatchGame::start(config, 1, 1);
    game.score(game.red[0]);
    assert!(!game.scoreboard.game_over);
    game.wait_for_play();
    game.score(game.red[0]);
    assert!(game.scoreboard.game_over);
}

#[test]
fn first_ends_the_game_when_a_team_reaches_enough_goals() {
    let config = MatchConfiguration {
        first_to: 2,
        ..MatchGame::quick_config()
    };
    let mut game = MatchGame::start(config, 1, 1);
    game.score(game.blue[0]);
    game.wait_for_play();
    game.score(game.red[0]);
    assert!(!game.scoreboard.game_over);
    game.wait_for_play();
    game.score(game.red[0]);
    assert!(game.scoreboard.game_over);
}

#[test]
fn use_mph_prints_puck_speeds_in_miles_per_hour() {
    for (use_mph, unit) in [(false, "km/h"), (true, "mph")] {
        let config = MatchConfiguration {
            use_mph,
            ..MatchGame::quick_config()
        };
        let mut game = MatchGame::start(config, 1, 1);
        game.score(game.red[0]);
        let message = game
            .heard
            .iter()
            .find(|chat| chat.starts_with("Goal scored"))
            .expect("a goal message");
        assert!(message.contains(unit), "{message}");
    }
}

#[test]
fn goal_replay_shows_a_replay_after_goals() {
    let config = MatchConfiguration {
        goal_replay: true,
        ..MatchGame::quick_config()
    };
    let mut game = MatchGame::start(config, 1, 1);
    game.score(game.red[0]);
    game.wait_to_hear(500, "Goal replay");
    game.tick();
    assert!(game.t.ctx().replay.is_in_replay());
}

// ----- Commands available for all -----

/// A server in match mode with players that are not on the ice.
fn match_server() -> TestServer<StandardMatchGameMode> {
    TestServer::new(StandardMatchGameMode::new(
        MatchGame::quick_config(),
        5,
        SpawnPoint::Center,
    ))
}

#[test]
fn team_chat_reaches_only_team_mates() {
    let mut game = MatchGame::start(MatchGame::quick_config(), 2, 1);
    let (red0, red1, blue) = (game.red[0], game.red[1], game.blue[0]);
    game.t.take_messages(red1);
    game.t.take_messages(blue);
    game.chat(red0, "/t go left");
    assert!(game.t.take_chats(red1).contains(&"go left".to_owned()));
    assert!(!game.t.take_chats(blue).contains(&"go left".to_owned()));
}

#[test]
fn list_shows_up_to_five_player_ids_from_an_optional_start() {
    let mut t = match_server();
    let players: Vec<_> = (0..7).map(|i| t.join(&format!("player{i}"))).collect();
    let asker = players[0];
    t.take_messages(asker);
    t.chat(asker, "/list");
    assert_eq!(
        t.take_chats(asker),
        [
            "0: player0",
            "1: player1",
            "2: player2",
            "3: player3",
            "4: player4"
        ]
    );
    t.chat(asker, "/list 5");
    assert_eq!(t.take_chats(asker), ["5: player5", "6: player6"]);
}

#[test]
fn search_lists_players_whose_name_contains_the_text() {
    let mut t = match_server();
    let asker = t.join("asker");
    t.join("MigoMipo");
    t.join("Other");
    t.take_messages(asker);
    t.chat(asker, "/search Mipo");
    assert_eq!(t.take_chats(asker), ["1: MigoMipo"]);
}

#[test]
fn view_enters_first_person_view_of_a_player() {
    let mut t = match_server();
    let viewer = t.join("viewer");
    t.join("target");
    t.take_messages(viewer);
    t.chat(viewer, "/view 1");
    assert_eq!(t.take_chats(viewer), ["You are now viewing target"]);
    t.chat(viewer, "/restoreview");
    assert_eq!(t.take_chats(viewer), ["View has been restored"]);
}

#[test]
#[ignore = "/view from the ice is refused with \"You must be a spectator to change view\", in the code before 2ec56b3 too"]
fn view_from_the_ice_makes_the_player_a_spectator() {
    let mut game = MatchGame::start(MatchGame::quick_config(), 1, 1);
    let viewer = game.red[0];
    game.chat(viewer, "/view 1");
    assert!(game.heard("You are now viewing blue0"), "{:?}", game.heard);
    assert!(game.t.ctx().objects.skater(viewer).is_none());
}

#[test]
fn views_finds_a_player_by_name_or_lists_the_matches() {
    let mut t = match_server();
    let viewer = t.join("viewer");
    t.join("target one");
    t.join("target two");
    t.take_messages(viewer);
    t.chat(viewer, "/views one");
    assert_eq!(t.take_chats(viewer), ["You are now viewing target one"]);
    t.chat(viewer, "/views target");
    assert_eq!(
        t.take_chats(viewer),
        [
            "Multiple matches found, use /view X",
            "1: target one",
            "2: target two"
        ]
    );
}

fn add_ping(t: &mut TestServer<impl GameMode>, player: PlayerId, seconds: f32) {
    let player = t
        .server
        .state
        .player_message_state
        .players
        .get_player_mut(player)
        .unwrap();
    let ServerPlayerData::NetworkPlayer { data } = &mut player.data else {
        panic!("expected a network player");
    };
    data.last_ping.push_front(seconds);
}

#[test]
fn ping_and_pings_show_a_players_server_side_ping() {
    let mut t = match_server();
    let asker = t.join("asker");
    let target = t.join("target");
    add_ping(&mut t, target, 0.05);
    t.take_messages(asker);
    t.chat(asker, "/ping 1");
    let chats = t.take_chats(asker);
    assert_eq!(chats[0], "target ping: avg 50 ms");
    t.chat(asker, "/pings targ");
    assert_eq!(t.take_chats(asker), chats);
}

fn hand(game: &MatchGame, player: PlayerId) -> SkaterHand {
    game.server_objects()
        .find_map(|object| match object {
            crate::server::GameObject::Skater(id, skater) if *id == player => Some(skater.hand),
            _ => None,
        })
        .unwrap()
}

#[test]
#[ignore = "since 2ec56b3, /lefty and /righty change the hand at once during play"]
fn lefty_and_righty_during_play_apply_after_play_has_stopped() {
    let config = MatchConfiguration {
        icing: IcingConfiguration::NoTouch,
        ..MatchGame::quick_config()
    };
    let mut game = MatchGame::start(config, 1, 1);
    let player = game.red[0];
    assert_eq!(hand(&game, player), SkaterHand::Right);
    game.chat(player, "/lefty");
    game.tick();
    assert_eq!(hand(&game, player), SkaterHand::Right);

    // Play stops for icing, and the hand changes at the next faceoff at the latest.
    ice_the_puck(&mut game);
    game.wait_for_play();
    assert_eq!(hand(&game, player), SkaterHand::Left);
}

#[test]
fn lefty_outside_of_play_applies_at_once() {
    let mut game = MatchGame::join(MatchGame::quick_config(), 1, 0);
    game.ticks(2);
    let player = game.red[0];
    game.chat(player, "/lefty");
    assert_eq!(hand(&game, player), SkaterHand::Left);
    game.chat(player, "/righty");
    assert_eq!(hand(&game, player), SkaterHand::Right);
}

#[test]
#[ignore = "/rules was removed in 2ec56b3"]
fn rules_shows_the_offside_and_icing_settings() {
    let mut t = match_server();
    let player = t.join("player");
    t.take_messages(player);
    t.chat(player, "/rules");
    let chats = t.take_chats(player);
    assert!(
        !chats.is_empty() && !chats.iter().any(|chat| chat.starts_with("Unknown command")),
        "{chats:?}"
    );
}

#[test]
fn admin_logs_in_with_the_right_password() {
    let mut t = match_server();
    t.server.config.password = Some("secret".to_owned());
    let player = t.join("player");
    t.take_messages(player);
    t.chat(player, "/admin wrong");
    assert_eq!(t.take_chats(player), ["Wrong administrator password"]);
    t.chat(player, "/admin secret");
    assert_eq!(
        t.take_chats(player),
        ["Successfully logged in as administrator"]
    );
    t.chat(player, "/mutechat");
    assert_eq!(t.take_chats(player), ["Chat muted by player"]);
}

// ----- Commands for administrators -----

#[test]
#[ignore = "/disablejoin and /enablejoin were removed in 2ec56b3"]
fn disablejoin_and_enablejoin_control_whether_new_players_can_join() {
    let mut t = match_server();
    let admin = t.join_admin("admin");
    t.chat(admin, "/disablejoin");
    assert!(t.try_join("blocked").is_none());
    t.chat(admin, "/enablejoin");
    assert!(t.try_join("welcome").is_some());
}

#[test]
fn kick_removes_a_player() {
    let mut t = match_server();
    let admin = t.join_admin("admin");
    let kicked = t.join("kicked");
    t.take_messages(admin);
    t.chat(admin, "/kick 1");
    assert!(t.ctx().players.get(kicked).is_none());
    assert!(
        t.take_chats(admin)
            .contains(&"kicked kicked by admin".to_owned())
    );
}

#[test]
fn ban_kicks_and_bans_until_clearbans() {
    let mut t = match_server();
    let admin = t.join_admin("admin");
    let banned = t.join("banned");
    t.chat(admin, "/ban 1");
    assert!(t.ctx().players.get(banned).is_none());
    // Test players all connect from 127.0.0.1, so nobody else can join while it is banned.
    assert!(t.try_join("again").is_none());
    t.chat(admin, "/clearbans");
    assert!(t.try_join("again").is_some());
}

#[test]
fn fs_forces_a_player_off_the_ice() {
    let mut game = MatchGame::start(MatchGame::quick_config(), 1, 1);
    let blue = game.blue[0];
    game.chat(game.admin(), &format!("/fs {}", blue.index));
    assert!(game.t.ctx().objects.skater(blue).is_none());
}

#[test]
fn mute_and_unmute_a_player() {
    let mut t = match_server();
    let admin = t.join_admin("admin");
    let player = t.join("player");
    t.chat(admin, "/mute 1");
    t.take_messages(admin);
    t.chat(player, "hello");
    assert!(!t.take_chats(admin).contains(&"hello".to_owned()));
    t.chat(admin, "/unmute 1");
    t.chat(player, "hello again");
    assert!(t.take_chats(admin).contains(&"hello again".to_owned()));
}

#[test]
fn mutechat_and_unmutechat_mute_all_chat() {
    let mut t = match_server();
    let admin = t.join_admin("admin");
    let player = t.join("player");
    t.chat(admin, "/mutechat");
    t.take_messages(admin);
    t.chat(player, "hello");
    assert!(!t.take_chats(admin).contains(&"hello".to_owned()));
    t.chat(admin, "/unmutechat");
    t.chat(player, "hello again");
    assert!(t.take_chats(admin).contains(&"hello again".to_owned()));
}

#[test]
#[ignore = "/start was removed in 2ec56b3"]
fn start_starts_the_game_during_warmup() {
    let mut game = MatchGame::join(MatchGame::quick_config(), 1, 1);
    game.ticks(5);
    assert_eq!(game.scoreboard.period, 0);
    game.chat(game.admin(), "/start");
    game.tick_until(500, |game| game.scoreboard.period >= 1);
}

#[test]
#[ignore = "/reset was removed from match mode in 2ec56b3"]
fn reset_resets_the_game() {
    let mut game = MatchGame::start(MatchGame::quick_config(), 1, 1);
    game.score(game.red[0]);
    game.chat(game.admin(), "/reset");
    game.ticks(2);
    assert_eq!(game.scoreboard.period, 0);
    assert_eq!(game.scoreboard.red_score, 0);
}

#[test]
#[ignore = "/pause and /unpause were removed from match mode in 2ec56b3"]
fn pause_and_unpause_stop_and_restart_the_clock() {
    let mut game = MatchGame::start(MatchGame::quick_config(), 1, 1);
    game.chat(game.admin(), "/pause");
    game.tick();
    let time = game.scoreboard.time;
    game.ticks(50);
    assert_eq!(game.scoreboard.time, time);
    game.chat(game.admin(), "/unpause");
    game.ticks(50);
    assert!(game.scoreboard.time < time);
}

#[test]
#[ignore = "/faceoff was removed in 2ec56b3"]
fn faceoff_calls_a_center_ice_faceoff() {
    let mut game = MatchGame::start(MatchGame::quick_config(), 1, 1);
    game.set_puck(Vec3::new(PUCK_LANE, 0.5, 10.0), Vec3::ZERO);
    game.chat(game.admin(), "/faceoff");
    game.wait_for_play();
    game.tick_until(1000, |game| {
        let puck = game.pucks()[0];
        (puck.x - 15.0).abs() < 0.5 && (puck.z - 30.5).abs() < 0.5
    });
}

#[test]
fn replay_turns_server_side_replays_on_and_off() {
    let mut t = match_server();
    let admin = t.join_admin("admin");
    t.take_messages(admin);
    t.chat(admin, "/replay on");
    assert_eq!(t.server.config.recording_enabled, ReplayRecording::On);
    t.chat(admin, "/replay off");
    assert_eq!(t.server.config.recording_enabled, ReplayRecording::Off);
    assert_eq!(
        t.take_chats(admin),
        ["Replays enabled by admin", "Replays disabled by admin"]
    );
}

#[test]
fn set_clock_period_and_scores() {
    let mut game = MatchGame::start(MatchGame::quick_config(), 1, 1);
    let admin = game.admin();
    game.chat(admin, "/set clock 1:30");
    game.chat(admin, "/set period 2");
    game.chat(admin, "/set redscore 3");
    game.chat(admin, "/set bluescore 4");
    game.tick();
    let scoreboard = &game.scoreboard;
    assert!(
        (8990..=9000).contains(&scoreboard.time),
        "{}",
        scoreboard.time
    );
    assert_eq!(scoreboard.period, 2);
    assert_eq!((scoreboard.red_score, scoreboard.blue_score), (3, 4));
}

#[test]
fn set_period_zero_returns_to_warmup() {
    let config = MatchConfiguration {
        icing: IcingConfiguration::NoTouch,
        ..MatchGame::quick_config()
    };
    let mut game = MatchGame::start(config, 1, 1);
    game.chat(game.admin(), "/set redscore 2");
    game.chat(game.admin(), "/set period 0");
    game.tick();
    assert_eq!(game.scoreboard.period, 0);
    assert_eq!(game.scoreboard.red_score, 2);
    // Warmup has no rules, so icing the puck is not called.
    ice_the_puck(&mut game);
    assert!(!game.heard("Icing"), "{:?}", game.heard);

    // The game starts again after warmup, with the score kept.
    game.play_until_faceoff();
    assert_eq!(game.scoreboard.period, 1);
    assert_eq!(game.scoreboard.red_score, 2);
}

#[test]
fn set_periodnum_sets_the_number_of_periods() {
    let config = MatchConfiguration {
        time_period: 5,
        ..MatchGame::quick_config()
    };
    let mut game = MatchGame::start(config, 1, 1);
    game.chat(game.admin(), "/set periodnum 1");
    game.score(game.red[0]);
    game.tick_until(1000, |game| game.scoreboard.period == 2);
    assert!(game.scoreboard.game_over);
}

#[test]
fn set_rule_settings_announce_the_new_rules() {
    let mut game = MatchGame::join(MatchGame::quick_config(), 1, 0);
    let admin = game.admin();
    let mut failures = vec![];
    for (command, message) in [
        ("icing on", "Touch icing enabled"),
        ("icing notouch", "No-touch icing enabled"),
        ("icing off", "Icing disabled"),
        ("offside on", "Offside enabled"),
        ("offside imm", "Immediate offside enabled"),
        ("offside off", "Offside disabled"),
        ("offside immediate", "Immediate offside enabled"),
        ("offside off", "Offside disabled"),
        ("offsideline center", "Center line set as offside line"),
        ("offsideline blue", "Blue line set as offside line"),
        ("twolinepass on", "Regular two-line pass rule enabled"),
        ("twolinepass forward", "Forward two-line pass rule enabled"),
        (
            "twolinepass both",
            "Regular and forward two-line pass rules enabled",
        ),
        ("twolinepass three", "Three-line pass rule enabled"),
        ("twolinepass off", "Two-line pass rule disabled"),
        ("mercy 3", "Mercy rule set to 3 goals"),
        ("mercy 0", "Mercy rule disabled"),
        ("first 5", "First-to-goals rule set to 5 goals"),
        ("first 0", "First-to-goals rule disabled"),
        ("goalreplay on", "Goal replays enabled"),
        ("goalreplay off", "Goal replays disabled"),
        ("teamsize 3", "Team size set to 3"),
    ] {
        game.heard.clear();
        game.chat(admin, &format!("/set {command}"));
        let expected = format!("{message} by red0");
        if !game.heard(&expected) {
            failures.push(format!("/set {command}: {:?}", game.heard));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn set_mercy_and_first_off_disable_the_rules() {
    let mut game = MatchGame::join(MatchGame::quick_config(), 1, 0);
    let admin = game.admin();
    for (enable, disable, message) in [
        ("mercy 3", "mercy off", "Mercy rule disabled by red0"),
        (
            "first 5",
            "first off",
            "First-to-goals rule disabled by red0",
        ),
    ] {
        game.chat(admin, &format!("/set {enable}"));
        game.heard.clear();
        game.chat(admin, &format!("/set {disable}"));
        assert!(game.heard(message), "/set {disable}: {:?}", game.heard);
    }
}

#[test]
fn set_icing_changes_the_icing_rule() {
    let mut game = MatchGame::start(MatchGame::quick_config(), 1, 1);
    game.chat(game.admin(), "/set icing notouch");
    ice_the_puck(&mut game);
    assert!(game.heard("Icing"), "{:?}", game.heard);
}

#[test]
fn set_teamsize_limits_the_players_on_each_team() {
    let mut game = MatchGame::join(MatchGame::quick_config(), 1, 0);
    game.chat(game.admin(), "/set teamsize 1");
    let second = game.t.join("second");
    game.t.press_join(second, Team::Red);
    game.ticks(5);
    assert_eq!(game.t.ctx().players.count_team_members(), (1, 0));
}

#[test]
fn set_spawn_settings_apply_at_the_next_faceoff() {
    let mut game = MatchGame::join(MatchGame::quick_config(), 1, 1);
    let admin = game.admin();
    game.chat(admin, "/set spawnoffset 4");
    game.chat(admin, "/set spawnplayeraltitude 4");
    game.chat(admin, "/set spawnpuckaltitude 3");
    game.play_until_faceoff();
    let center_z = game.t.server.rink.center_line.z;
    let red = game.skater_pos(game.red[0]);
    assert!(((red.z - center_z) - 4.0).abs() < 0.1, "{red}");
    assert!((red.y - 4.0).abs() < 0.1, "{red}");
    assert!((game.pucks()[0].y - 3.0).abs() < 0.1);
}

/// Kicks with `/kickall pattern` from a server with MigoMipo and Other, and returns the patterns
/// that did not kick exactly MigoMipo.
fn kickall_failures(patterns: &[&'static str]) -> Vec<&'static str> {
    let mut failures = vec![];
    for &pattern in patterns {
        let mut t = match_server();
        let admin = t.join_admin("admin");
        let target = t.join("MigoMipo");
        let other = t.join("Other");
        t.chat(admin, &format!("/kickall {pattern}"));
        if t.ctx().players.get(target).is_some() || t.ctx().players.get(other).is_none() {
            failures.push(pattern);
        }
    }
    failures
}

#[test]
fn kickall_kicks_players_with_the_exact_name() {
    assert_eq!(kickall_failures(&["MigoMipo"]), Vec::<&str>::new());
}

#[test]
#[ignore = "kickall matches case-sensitively and its wildcards are reversed (migo% matches names             ending in migo), in the code before 2ec56b3 too"]
fn kickall_ignores_case_and_supports_wildcards() {
    let patterns = ["migomipo", "Migo%", "migo%", "%Mipo", "%mipo", "%gomi%"];
    let failures = kickall_failures(&patterns);
    assert!(
        failures.is_empty(),
        "patterns that did not kick only MigoMipo: {failures:?}"
    );
}

#[test]
fn banall_kicks_and_bans_players_by_name() {
    let mut t = match_server();
    let admin = t.join_admin("admin");
    let target = t.join("MigoMipo");
    t.chat(admin, "/banall MigoMipo");
    assert!(t.ctx().players.get(target).is_none());
    assert!(t.try_join("again").is_none());
}

// ----- Other modes -----

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
fn shootout_attempts_sets_the_attempts_per_team() {
    let mut t = TestServer::new(ShootoutGameMode::new(ShootoutGameConfiguration {
        attempts: 3,
    }));
    let red = t.join("red");
    let blue = t.join("blue");
    t.press_join(red, Team::Red);
    t.press_join(blue, Team::Blue);
    t.take_messages(red);
    for _ in 0..2000 {
        t.tick();
    }
    assert!(
        t.take_chats(red)
            .contains(&"3 attempts left for Red".to_owned())
    );
}

use std::collections::{HashMap, HashSet, VecDeque};

use glam::Vec3;
use glamx::Rot3;

use crate::game::{
    PhysicsEvent, PlayerId, PlayerIndex, Puck, PuckId, RulesState, ScoreboardValues, Team,
};
use crate::gamemode::util::{GameProgress, SpawnPoint, TeamSelection, get_spawnpoint};
use crate::gamemode::{CommandResult, ExitReason, GameContext, GameMode};

mod commands;
mod faceoff;

use faceoff::{assign_team_positions, faceoff_spot};

/// Ticks of play before a goal that the goal replay shows, at most. The replay never starts
/// before the last faceoff.
const REPLAY_BEFORE_GOAL: u32 = 600;
/// Ticks of play after a goal that the goal replay shows. Play goes on live during these
/// ticks, and the replay starts after them.
const REPLAY_AFTER_GOAL: u32 = 200;
/// Minimum ticks between the end of a goal replay and the next faceoff.
const MIN_PAUSE_AFTER_REPLAY: u32 = 300;
/// Ticks of game history kept for replays. Must cover the longest goal replay.
const REPLAY_HISTORY_LENGTH: u32 = 1000;
const _: () = assert!(REPLAY_BEFORE_GOAL + REPLAY_AFTER_GOAL < REPLAY_HISTORY_LENGTH);

pub const ALLOWED_POSITIONS: [&str; 18] = [
    "C", "LW", "RW", "LD", "RD", "G", "LM", "RM", "LLM", "RRM", "LLD", "RRD", "CM", "CD", "LW2",
    "RW2", "LLW", "RRW",
];

#[derive(Eq, PartialEq, Debug, Copy, Clone)]
pub enum IcingConfiguration {
    Off,
    Touch,
    NoTouch,
}
#[derive(Eq, PartialEq, Debug, Copy, Clone)]
pub enum OffsideConfiguration {
    Off,
    Delayed,
    Immediate,
}
#[derive(Eq, PartialEq, Debug, Copy, Clone)]
pub enum TwoLinePassConfiguration {
    Off,
    On,
    Forward,
    Double,
    ThreeLine,
}
#[derive(Eq, PartialEq, Debug, Copy, Clone)]
pub enum OffsideLineConfiguration {
    OffensiveBlue,
    Center,
}

#[derive(PartialEq, Debug, Copy, Clone)]
pub struct MatchConfiguration {
    pub time_period: u32,
    pub time_break: u32,
    pub time_intermission: u32,
    pub mercy: u32,
    pub first_to: u32,
    pub periods: u32,
    pub offside: OffsideConfiguration,
    pub icing: IcingConfiguration,
    pub offside_line: OffsideLineConfiguration,
    pub twoline_pass: TwoLinePassConfiguration,
    pub warmup_pucks: usize,
    pub use_mph: bool,
    pub goal_replay: bool,
    pub spawn_point_offset: f32,
    pub spawn_player_altitude: f32,
    pub spawn_puck_altitude: f32,
    pub spawn_keep_stick_position: bool,
}
impl Default for MatchConfiguration {
    fn default() -> Self {
        Self {
            time_period: 300,
            time_break: 10,
            time_intermission: 20,
            mercy: 0,
            first_to: 0,
            periods: 3,
            offside: OffsideConfiguration::Off,
            icing: IcingConfiguration::Off,
            offside_line: OffsideLineConfiguration::OffensiveBlue,
            twoline_pass: TwoLinePassConfiguration::Off,
            warmup_pucks: 1,
            use_mph: false,
            goal_replay: false,
            spawn_point_offset: 2.75,
            spawn_player_altitude: 2.75,
            spawn_puck_altitude: 1.5,
            spawn_keep_stick_position: false,
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
enum RinkSide {
    Lower,
    Higher,
}
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
enum Faceoff {
    Center,
    Defensive(Team, RinkSide),
    Offside(Team, RinkSide),
}
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum PassLocation {
    ReachedOwnBlue,
    PassedOwnBlue,
    ReachedCenter,
    PassedCenter,
    ReachedOffensive,
    PassedOffensive,
}
#[derive(Debug, Copy, Clone)]
struct Pass {
    team: Team,
    side: RinkSide,
    from: Option<PassLocation>,
    player: PlayerId,
}
#[derive(Debug, Copy, Clone)]
enum Icing {
    No,
    Warning(IcingWarning),
}
#[derive(Debug, Copy, Clone)]
struct IcingWarning {
    icing_team: Team,
    side: RinkSide,
}
#[derive(Debug, Copy, Clone)]
enum Offside {
    Neutral,
    InZone(Team),
    Warning(OffsideWarning),
}
#[derive(Debug, Copy, Clone)]
struct OffsideWarning {
    offending_team: Team,
    side: RinkSide,
    pass_origin: Option<PassLocation>,
    passer: PlayerId,
}
#[derive(Debug, Clone)]
enum TwoLine {
    No,
    Warning(TwoLineWarning),
}
#[derive(Debug, Clone)]
struct TwoLineWarning {
    passing_team: Team,
    side: RinkSide,
    pass_origin: PassLocation,
    twoline_offside_players: Vec<PlayerId>,
}
#[derive(Debug, Clone)]
struct PuckTouch {
    player: PlayerId,
    team: Team,
    puck_speed: f32,
    first_time: u32,
    last_time: u32,
}

#[derive(Debug)]
enum MatchState {
    Playing(PlayingState),
    Break(BreakState),
}
#[derive(Debug, Clone)]
struct BreakState {
    timer: u32,
    kind: BreakKind,
}
#[derive(Debug, Clone)]
enum BreakKind {
    Stoppage(Stoppage),
    Intermission {
        period_end_step: Option<u32>,
        too_late_printed: bool,
    },
    GameOver,
}
#[derive(Debug, Clone)]
struct Stoppage {
    cause: StoppageCause,
    next_faceoff: Faceoff,
}
#[derive(Debug, Clone)]
enum StoppageCause {
    Goal,
    Offside,
    Icing,
}
/// A change to the state of a running match.
///
/// The current state decides which transition to make, and [Match::apply] carries it out.
#[derive(Debug, Clone)]
enum MatchTransition {
    Continue,
    /// Stops play, or replaces the current break with another one.
    Break(BreakState),
    /// The period clock has run out.
    EndPeriod,
    StartPlaying {
        faceoff: Faceoff,
        reset_clock: bool,
    },
    /// The game is over and a new one should start.
    Finish,
}
#[derive(Debug)]
struct PlayingState {
    icing: Icing,
    offside: Offside,
    twoline: TwoLine,
    pass: Option<Pass>,
    started_goalies: HashSet<PlayerId>,
    puck_touches: HashMap<PuckId, VecDeque<PuckTouch>>,
    faceoff_step: u32,
}
impl BreakState {
    fn before_tick(&mut self, config: &MatchConfiguration, game_over: bool) -> MatchTransition {
        self.timer = self.timer.saturating_sub(1);
        if self.timer != 0 {
            return MatchTransition::Continue;
        }
        match &self.kind {
            BreakKind::Stoppage(_) if game_over => MatchTransition::Break(BreakState {
                timer: config.time_intermission * 100,
                kind: BreakKind::GameOver,
            }),
            BreakKind::Stoppage(stoppage) => MatchTransition::StartPlaying {
                faceoff: stoppage.next_faceoff,
                reset_clock: false,
            },
            BreakKind::Intermission { .. } => MatchTransition::StartPlaying {
                faceoff: Faceoff::Center,
                reset_clock: true,
            },
            BreakKind::GameOver => MatchTransition::Finish,
        }
    }
    fn scoreboard(&self) -> (RulesState, u32, bool) {
        match self.kind {
            BreakKind::Stoppage(Stoppage {
                cause: StoppageCause::Goal,
                ..
            }) => (RulesState::default(), self.timer, false),
            BreakKind::Stoppage(Stoppage {
                cause: StoppageCause::Offside,
                ..
            }) => (RulesState::Offside, 0, false),
            BreakKind::Stoppage(Stoppage {
                cause: StoppageCause::Icing,
                ..
            }) => (RulesState::Icing, 0, false),
            BreakKind::Intermission { .. } => (RulesState::default(), 0, false),
            BreakKind::GameOver => (RulesState::default(), 0, true),
        }
    }
    fn handle_events(&mut self, ctx: &mut GameContext, events: &[PhysicsEvent]) {
        let BreakKind::Intermission {
            period_end_step: Some(period_end_step),
            too_late_printed,
        } = &mut self.kind
        else {
            return;
        };
        if *too_late_printed
            || !events
                .iter()
                .any(|event| matches!(event, PhysicsEvent::PuckEnteredNet { .. }))
        {
            return;
        }
        let elapsed = ctx.replay.game_step().saturating_sub(*period_end_step);
        if elapsed <= 300 {
            *too_late_printed = true;
            ctx.messages.add_server_chat_message(format!(
                "{}.{:02} seconds too late!",
                elapsed / 100,
                elapsed % 100,
            ));
        }
    }
}
pub struct Match {
    state: MatchState,
    pub(crate) game_state: MatchGameState,
}
pub(crate) struct MatchGameState {
    pub(crate) score: (u32, u32),
    pub(crate) period: u32,
    pub(crate) time: u32,
    replay: Option<(u32, u32, Option<PlayerId>)>,
}
impl MatchGameState {
    fn game_over(&self, config: &MatchConfiguration) -> bool {
        (self.period > config.periods && self.score.0 != self.score.1)
            || (config.mercy > 0 && self.score.0.abs_diff(self.score.1) >= config.mercy)
            || (config.first_to > 0
                && (self.score.0 >= config.first_to || self.score.1 >= config.first_to))
    }
}

/// Settings of the standard match game mode that last across games.
///
/// The game mode owns these, and passes them to the running [Match] on every call.
pub struct MatchSettings {
    pub config: MatchConfiguration,
    /// Preferred faceoff position of each player.
    preferred_positions: HashMap<PlayerId, &'static str>,
}

impl MatchSettings {
    pub fn new(config: MatchConfiguration) -> Self {
        Self {
            config,
            preferred_positions: HashMap::new(),
        }
    }
}

impl Match {
    fn start(config: &MatchConfiguration, score: (u32, u32)) -> Self {
        Self {
            state: MatchState::Break(BreakState {
                timer: config.time_intermission * 100,
                kind: BreakKind::Intermission {
                    period_end_step: None,
                    too_late_printed: false,
                },
            }),
            game_state: MatchGameState {
                time: 0,
                score,
                period: 1,
                replay: None,
            },
        }
    }
    fn setup_faceoff(
        ctx: &mut GameContext,
        config: &MatchConfiguration,
        preferred_positions: &HashMap<PlayerId, &'static str>,
        faceoff: Faceoff,
    ) -> PlayingState {
        ctx.objects.remove_all_pucks();
        let rink = &*ctx.rink;
        let spot = faceoff_spot(
            rink,
            faceoff,
            config.spawn_point_offset,
            config.spawn_player_altitude,
        );
        let puck_pos = spot.center + Vec3::Y * config.spawn_puck_altitude;
        ctx.objects.spawn_puck(Puck::new(puck_pos, Rot3::IDENTITY));
        let mut started_goalies = HashSet::new();
        let mut red_players = Vec::new();
        let mut blue_players = Vec::new();
        let players: Vec<_> = ctx
            .players
            .iter()
            .filter_map(|player| {
                let team = player.team()?;
                match team {
                    Team::Red => red_players.push(player.id),
                    Team::Blue => blue_players.push(player.id),
                }
                Some((player.id, team))
            })
            .collect();
        let mut assignments = assign_team_positions(&red_players, preferred_positions);
        assignments.extend(assign_team_positions(&blue_players, preferred_positions));
        for (id, team) in players {
            let position = assignments[&id];
            let (pos, rot) = match team {
                Team::Red => spot.red[position],
                Team::Blue => spot.blue[position],
            };
            ctx.spawn_skater(id, team, pos, rot, config.spawn_keep_stick_position);
            if position == "G" {
                started_goalies.insert(id);
            }
        }
        PlayingState {
            icing: Icing::No,
            offside: Offside::Neutral,
            twoline: TwoLine::No,
            pass: None,
            started_goalies,
            puck_touches: HashMap::new(),
            faceoff_step: ctx.replay.game_step(),
        }
    }
}

/// Ticks of stoppage timer after a goal.
///
/// The stoppage timer stands still while a replay is shown, so with a replay the pause from the
/// goal to the faceoff is the timer plus the replay length. The timer is chosen so that the pause
/// is at least `time_break` seconds, and so that at least [MIN_PAUSE_AFTER_REPLAY] ticks are left
/// after the replay, which starts [REPLAY_AFTER_GOAL] ticks after the goal.
fn goal_stoppage_time(time_break: u32, replay_length: Option<u32>) -> u32 {
    let break_time = time_break * 100;
    match replay_length {
        None => break_time,
        Some(replay_length) => break_time
            .saturating_sub(replay_length)
            .max(REPLAY_AFTER_GOAL + MIN_PAUSE_AFTER_REPLAY),
    }
}

impl PlayingState {
    fn players_past(
        ctx: &GameContext,
        team: Team,
        ignore: Option<PlayerId>,
        center: bool,
    ) -> Vec<PlayerId> {
        let line = if center {
            &ctx.rink.center_line
        } else {
            match team {
                Team::Red => &ctx.rink.blue_zone_blue_line,
                Team::Blue => &ctx.rink.red_zone_blue_line,
            }
        };
        ctx.objects
            .skaters()
            .filter(|&(id, skater)| {
                let pos = skater.feet_position();
                Some(id) != ignore
                    && ctx.players.get(id).and_then(|p| p.team()) == Some(team)
                    && match team {
                        Team::Red => pos.z < line.z - line.width,
                        Team::Blue => pos.z > line.z + line.width / 2.0,
                    }
            })
            .map(|(id, _)| id)
            .collect()
    }
    fn team_in_zone(ctx: &GameContext, team: Team, ignore: Option<PlayerId>) -> bool {
        !Self::players_past(ctx, team, ignore, false).is_empty()
    }
    fn side(ctx: &GameContext, puck: PuckId) -> RinkSide {
        ctx.objects.get_puck(puck).map_or(RinkSide::Lower, |p| {
            if p.body.pos.x <= ctx.rink.width / 2.0 {
                RinkSide::Lower
            } else {
                RinkSide::Higher
            }
        })
    }
    fn stoppage(timer: u32, cause: StoppageCause, next_faceoff: Faceoff) -> BreakState {
        BreakState {
            timer,
            kind: BreakKind::Stoppage(Stoppage {
                cause,
                next_faceoff,
            }),
        }
    }
    fn call_offside(
        config: &MatchConfiguration,
        ctx: &mut GameContext,
        team: Team,
        side: RinkSide,
        from: Option<PassLocation>,
        self_touch: bool,
    ) -> BreakState {
        let next_faceoff = if self_touch {
            if config.offside_line == OffsideLineConfiguration::Center {
                Faceoff::Center
            } else {
                Faceoff::Offside(team.get_other_team(), side)
            }
        } else if from.is_some_and(|p| p <= PassLocation::ReachedOwnBlue) {
            Faceoff::Defensive(team, side)
        } else if from.is_some_and(|p| p <= PassLocation::ReachedCenter) {
            Faceoff::Offside(team, side)
        } else {
            Faceoff::Center
        };
        ctx.messages.add_server_chat_message("Offside");
        Self::stoppage(
            config.time_break * 100,
            StoppageCause::Offside,
            next_faceoff,
        )
    }
    fn call_icing(
        config: &MatchConfiguration,
        ctx: &mut GameContext,
        team: Team,
        side: RinkSide,
    ) -> BreakState {
        let next_faceoff = Faceoff::Defensive(team, side);
        ctx.messages.add_server_chat_message("Icing");
        Self::stoppage(config.time_break * 100, StoppageCause::Icing, next_faceoff)
    }
    fn call_twoline(
        config: &MatchConfiguration,
        ctx: &mut GameContext,
        team: Team,
        side: RinkSide,
        from: PassLocation,
    ) -> BreakState {
        let next_faceoff = if from <= PassLocation::ReachedOwnBlue {
            Faceoff::Defensive(team, side)
        } else if from <= PassLocation::ReachedCenter {
            Faceoff::Offside(team, side)
        } else {
            Faceoff::Center
        };
        ctx.messages.add_server_chat_message("Two-line pass");
        Self::stoppage(
            config.time_break * 100,
            StoppageCause::Offside,
            next_faceoff,
        )
    }
    fn goal(
        game_state: &mut MatchGameState,
        config: &MatchConfiguration,
        ctx: &mut GameContext,
        playing: &mut PlayingState,
        team: Team,
        puck: PuckId,
    ) -> BreakState {
        if let Offside::Warning(warning) = playing.offside {
            if warning.offending_team == team {
                return Self::call_offside(
                    config,
                    ctx,
                    team,
                    warning.side,
                    warning.pass_origin,
                    false,
                );
            }
        }
        let puck_speed_across_line = ctx
            .objects
            .get_puck(puck)
            .map_or(0.0, |puck| puck.body.linear_velocity.length());
        let (goal_scorer, assist, puck_speed_from_stick, last_touch) = {
            let mut goal_scorer = None;
            let mut assist = None;
            let mut scorer_first_touch = 0;
            let mut puck_speed_from_stick = None;
            let last_touch = playing
                .puck_touches
                .get(&puck)
                .and_then(|touches| touches.front().map(|touch| touch.player));
            if let Some(touches) = playing.puck_touches.get(&puck) {
                for touch in touches {
                    if goal_scorer.is_none() && touch.team == team {
                        goal_scorer = Some(touch.player);
                        scorer_first_touch = touch.first_time;
                        puck_speed_from_stick = Some(touch.puck_speed);
                    } else if touch.team == team {
                        if Some(touch.player) == goal_scorer {
                            scorer_first_touch = touch.first_time;
                        } else if touch.last_time.saturating_sub(scorer_first_touch) <= 1000 {
                            assist = Some(touch.player);
                            break;
                        }
                    }
                }
            }
            (goal_scorer, assist, puck_speed_from_stick, last_touch)
        };
        match team {
            Team::Red => game_state.score.0 += 1,
            Team::Blue => game_state.score.1 += 1,
        };
        ctx.add_goal_message(team, goal_scorer, assist);
        let unit = if config.use_mph { "mph" } else { "km/h" };
        let speed_factor = if config.use_mph { 223.693 } else { 360.0 };
        let from_stick = puck_speed_from_stick
            .map(|speed| format!(", {:.1} {unit} from stick", speed * speed_factor))
            .unwrap_or_default();
        ctx.messages.add_server_chat_message(format!(
            "Goal scored, {:.1} {unit} across line{from_stick}",
            puck_speed_across_line * speed_factor,
        ));
        if game_state.time < 1000 {
            ctx.messages.add_server_chat_message(format!(
                "{}.{:02} seconds left",
                game_state.time / 100,
                game_state.time % 100,
            ));
        }
        let replay_length = config.goal_replay.then(|| {
            let now = ctx.replay.game_step();
            let start = playing
                .faceoff_step
                .max(now.saturating_sub(REPLAY_BEFORE_GOAL));
            let end = now + REPLAY_AFTER_GOAL;
            game_state.replay = Some((start, end, goal_scorer.or(last_touch)));
            end - start + 1
        });
        let stoppage_time = goal_stoppage_time(config.time_break, replay_length);
        Self::stoppage(stoppage_time, StoppageCause::Goal, Faceoff::Center)
    }
    fn touch(
        game_state: &MatchGameState,
        config: &MatchConfiguration,
        ctx: &mut GameContext,
        playing: &mut PlayingState,
        player: PlayerId,
        puck: PuckId,
    ) -> Option<BreakState> {
        let Some(team) = ctx.players.get(player).and_then(|p| p.team()) else {
            return None;
        };
        let side = Self::side(ctx, puck);
        if let Some(puck_object) = ctx.objects.get_puck(puck) {
            let touches = playing.puck_touches.entry(puck).or_default();
            if let Some(last_touch) = touches
                .front_mut()
                .filter(|touch| touch.player == player && touch.team == team)
            {
                last_touch.last_time = game_state.time;
                last_touch.puck_speed = puck_object.body.linear_velocity.length();
            } else {
                touches.truncate(15);
                touches.push_front(PuckTouch {
                    player,
                    team,
                    puck_speed: puck_object.body.linear_velocity.length(),
                    first_time: game_state.time,
                    last_time: game_state.time,
                });
            }
        }
        playing.pass = Some(Pass {
            team,
            side,
            from: None,
            player,
        });
        if let Offside::Warning(warning) = playing.offside {
            if warning.offending_team == team {
                return Some(Self::call_offside(
                    config,
                    ctx,
                    team,
                    warning.side,
                    warning.pass_origin,
                    player == warning.passer,
                ));
            }
        }
        if let TwoLine::Warning(warning) = playing.twoline.clone() {
            if warning.passing_team == team && warning.twoline_offside_players.contains(&player) {
                return Some(Self::call_twoline(
                    config,
                    ctx,
                    team,
                    warning.side,
                    warning.pass_origin,
                ));
            }
            playing.twoline = TwoLine::No;
            ctx.messages
                .add_server_chat_message("Two-line pass waved off");
        }
        if let Icing::Warning(warning) = playing.icing {
            if team != warning.icing_team && !playing.started_goalies.contains(&player) {
                return Some(Self::call_icing(
                    config,
                    ctx,
                    warning.icing_team,
                    warning.side,
                ));
            } else {
                playing.icing = Icing::No;
                ctx.messages.add_server_chat_message("Icing waved off");
            }
        }
        None
    }
    fn update_pass(playing: &mut PlayingState, team: Team, at: PassLocation) {
        if let Some(pass) = &mut playing.pass {
            if pass.team == team && pass.from.is_none() {
                pass.from = Some(at);
            }
        }
    }
    fn wave_off_twoline(ctx: &mut GameContext, playing: &mut PlayingState, team: Team) {
        if let TwoLine::Warning(warning) = &playing.twoline {
            if warning.passing_team != team {
                playing.twoline = TwoLine::No;
                ctx.messages
                    .add_server_chat_message("Two-line pass waved off");
            }
        }
    }
    fn enter_zone(
        config: &MatchConfiguration,
        ctx: &mut GameContext,
        playing: &mut PlayingState,
        team: Team,
        blue_line: bool,
    ) -> Option<BreakState> {
        let enabled = (blue_line && config.offside_line == OffsideLineConfiguration::OffensiveBlue)
            || (!blue_line && config.offside_line == OffsideLineConfiguration::Center);
        if enabled && !matches!(playing.offside, Offside::InZone(t) if t == team) {
            if let Some(pass) = playing.pass {
                if pass.team == team && Self::team_in_zone(ctx, team, Some(pass.player)) {
                    match config.offside {
                        OffsideConfiguration::Delayed => {
                            playing.offside = Offside::Warning(OffsideWarning {
                                offending_team: team,
                                side: pass.side,
                                pass_origin: pass.from,
                                passer: pass.player,
                            });
                            ctx.messages.add_server_chat_message("Offside warning");
                        }
                        OffsideConfiguration::Immediate => {
                            return Some(Self::call_offside(
                                config, ctx, team, pass.side, pass.from, false,
                            ));
                        }
                        OffsideConfiguration::Off => playing.offside = Offside::InZone(team),
                    }
                } else {
                    playing.offside = Offside::InZone(team);
                }
            } else {
                playing.offside = Offside::InZone(team);
            }
        }
        if let Some(pass) = playing.pass {
            if pass.team == team && Self::twoline_is_active(config, blue_line, pass.from) {
                let twoline_offside_players =
                    Self::players_past(ctx, team, Some(pass.player), !blue_line);
                if !twoline_offside_players.is_empty() {
                    playing.twoline = TwoLine::Warning(TwoLineWarning {
                        passing_team: team,
                        side: pass.side,
                        pass_origin: pass.from.unwrap(),
                        twoline_offside_players,
                    });
                    ctx.messages
                        .add_server_chat_message("Two-line pass warning");
                }
            }
        }
        None
    }
    fn twoline_is_active(
        config: &MatchConfiguration,
        blue_line: bool,
        from: Option<PassLocation>,
    ) -> bool {
        let Some(from) = from else { return false };
        match (blue_line, config.twoline_pass) {
            (false, TwoLinePassConfiguration::On | TwoLinePassConfiguration::Double) => {
                from <= PassLocation::ReachedOwnBlue
            }
            (true, TwoLinePassConfiguration::Forward | TwoLinePassConfiguration::Double) => {
                from <= PassLocation::ReachedCenter
            }
            (true, TwoLinePassConfiguration::ThreeLine) => from <= PassLocation::ReachedOwnBlue,
            _ => false,
        }
    }
    fn events(
        game_state: &mut MatchGameState,
        config: &MatchConfiguration,
        ctx: &mut GameContext,
        events: &[PhysicsEvent],
        playing: &mut PlayingState,
    ) -> MatchTransition {
        for event in events {
            match *event {
                PhysicsEvent::PuckTouch { player, puck } => {
                    if let Some(stoppage) =
                        Self::touch(game_state, config, ctx, playing, player, puck)
                    {
                        return MatchTransition::Break(stoppage);
                    }
                }
                PhysicsEvent::PuckEnteredNet { team, puck } => {
                    return MatchTransition::Break(Self::goal(
                        game_state,
                        config,
                        ctx,
                        playing,
                        team.get_other_team(),
                        puck,
                    ));
                }
                PhysicsEvent::PuckPassedGoalLine { team, puck: _ } => {
                    if let Some(pass) = playing.pass {
                        if pass.team == team.get_other_team()
                            && pass.from.is_some_and(|p| p <= PassLocation::ReachedCenter)
                        {
                            match config.icing {
                                IcingConfiguration::Touch => {
                                    playing.icing = Icing::Warning(IcingWarning {
                                        icing_team: pass.team,
                                        side: pass.side,
                                    });
                                    ctx.messages.add_server_chat_message("Icing warning");
                                }
                                IcingConfiguration::NoTouch => {
                                    return MatchTransition::Break(Self::call_icing(
                                        config, ctx, pass.team, pass.side,
                                    ));
                                }
                                IcingConfiguration::Off => {}
                            }
                        }
                    }
                }
                PhysicsEvent::PuckReachedDefensiveLine { team, .. } => {
                    Self::wave_off_twoline(ctx, playing, team);
                    Self::update_pass(playing, team, PassLocation::ReachedOwnBlue)
                }
                PhysicsEvent::PuckPassedDefensiveLine { team, .. } => {
                    Self::update_pass(playing, team, PassLocation::PassedOwnBlue);
                    if config.offside_line == OffsideLineConfiguration::OffensiveBlue {
                        if let Offside::Warning(warning) = playing.offside {
                            if team.get_other_team() == warning.offending_team {
                                ctx.messages.add_server_chat_message("Offside waved off");
                            }
                        }
                        playing.offside = Offside::Neutral;
                    }
                }
                PhysicsEvent::PuckReachedCenterLine { team, .. } => {
                    Self::wave_off_twoline(ctx, playing, team);
                    Self::update_pass(playing, team, PassLocation::ReachedCenter)
                }
                PhysicsEvent::PuckPassedCenterLine { team, .. } => {
                    Self::update_pass(playing, team, PassLocation::PassedCenter);
                    if let Some(stoppage) = Self::enter_zone(config, ctx, playing, team, false) {
                        return MatchTransition::Break(stoppage);
                    }
                    if let Offside::Warning(warning) = playing.offside {
                        if warning.offending_team != team {
                            ctx.messages.add_server_chat_message("Offside waved off");
                        }
                    }
                }
                PhysicsEvent::PuckReachedOffensiveZone { team, .. } => {
                    Self::update_pass(playing, team, PassLocation::ReachedOffensive)
                }
                PhysicsEvent::PuckEnteredOffensiveZone { team, .. } => {
                    Self::update_pass(playing, team, PassLocation::PassedOffensive);
                    if let Some(stoppage) = Self::enter_zone(config, ctx, playing, team, true) {
                        return MatchTransition::Break(stoppage);
                    }
                }
                _ => {}
            }
        }
        if let Offside::Warning(warning) = playing.offside {
            if !Self::team_in_zone(ctx, warning.offending_team, None) {
                playing.offside = Offside::InZone(warning.offending_team);
                ctx.messages.add_server_chat_message("Offside waved off");
            }
        }
        MatchTransition::Continue
    }
}

impl Match {
    fn before_tick(&mut self, ctx: &mut GameContext, settings: &MatchSettings) -> GameProgress {
        // Both the period clock and the stoppage timer stand still while a replay is shown, so
        // a goal replay neither uses up game time nor the pause before the next faceoff.
        if ctx.replay.is_in_replay() {
            return GameProgress::Continue;
        }
        let transition = match &mut self.state {
            MatchState::Playing(_) => {
                self.game_state.time = self.game_state.time.saturating_sub(1);
                if self.game_state.time == 0 {
                    MatchTransition::EndPeriod
                } else {
                    MatchTransition::Continue
                }
            }
            MatchState::Break(state) => state.before_tick(
                &settings.config,
                self.game_state.game_over(&settings.config),
            ),
        };
        self.apply(ctx, settings, transition)
    }
    /// Carries out a transition. This is the only place where a running match changes state.
    fn apply(
        &mut self,
        ctx: &mut GameContext,
        settings: &MatchSettings,
        transition: MatchTransition,
    ) -> GameProgress {
        match transition {
            MatchTransition::Continue => {}
            MatchTransition::Break(state) => self.state = MatchState::Break(state),
            MatchTransition::EndPeriod => {
                self.game_state.period += 1;
                self.state = MatchState::Break(BreakState {
                    timer: settings.config.time_intermission * 100,
                    kind: if self.game_state.game_over(&settings.config) {
                        BreakKind::GameOver
                    } else {
                        BreakKind::Intermission {
                            period_end_step: Some(ctx.replay.game_step()),
                            too_late_printed: false,
                        }
                    },
                });
            }
            MatchTransition::StartPlaying {
                faceoff,
                reset_clock,
            } => {
                if reset_clock {
                    self.game_state.time = settings.config.time_period * 100;
                }
                self.state = MatchState::Playing(Self::setup_faceoff(
                    ctx,
                    &settings.config,
                    &settings.preferred_positions,
                    faceoff,
                ));
            }
            MatchTransition::Finish => return GameProgress::Finished,
        }
        GameProgress::Continue
    }
    fn scoreboard(&self) -> ScoreboardValues {
        let (rules_state, goal_message_timer, game_over) = match &self.state {
            MatchState::Break(state) => state.scoreboard(),
            MatchState::Playing(playing) => (
                RulesState::Regular {
                    offside_warning: matches!(playing.offside, Offside::Warning(..))
                        || matches!(playing.twoline, TwoLine::Warning(..)),
                    icing_warning: matches!(playing.icing, Icing::Warning(..)),
                },
                0,
                false,
            ),
        };
        ScoreboardValues {
            rules_state,
            red_score: self.game_state.score.0,
            blue_score: self.game_state.score.1,
            period: self.game_state.period,
            time: self.game_state.time,
            goal_message_timer,
            game_over,
        }
    }
    fn after_tick(
        &mut self,
        ctx: &mut GameContext,
        settings: &MatchSettings,
        events: &[PhysicsEvent],
    ) -> GameProgress {
        let transition = match &mut self.state {
            MatchState::Playing(playing) => {
                PlayingState::events(&mut self.game_state, &settings.config, ctx, events, playing)
            }
            MatchState::Break(state) => {
                state.handle_events(ctx, events);
                MatchTransition::Continue
            }
        };
        let progress = self.apply(ctx, settings, transition);
        if let Some((start_step, end_step, force_view)) = self.game_state.replay
            && end_step <= ctx.replay.game_step()
        {
            self.game_state.replay = None;
            self.queue_goal_replay(ctx, &settings.config, start_step, end_step, force_view);
        }
        progress
    }
    /// Queues the goal replay once all of its ticks have been played.
    ///
    /// The goal stoppage timer was set for the full replay length. If part of the replay is
    /// missing from the history, the timer is changed to what [goal_stoppage_time] gives for the
    /// ticks that were queued, or for no replay if none were.
    fn queue_goal_replay(
        &mut self,
        ctx: &mut GameContext,
        config: &MatchConfiguration,
        start_step: u32,
        end_step: u32,
        force_view: Option<PlayerId>,
    ) {
        let queued = ctx
            .replay
            .add_replay_to_queue(start_step, end_step, force_view);
        if queued > 0 {
            ctx.messages.add_server_chat_message("Goal replay");
        }
        let planned = goal_stoppage_time(config.time_break, Some(end_step - start_step + 1));
        let actual = goal_stoppage_time(config.time_break, (queued > 0).then_some(queued));
        if let MatchState::Break(state) = &mut self.state {
            // The timer has been running since the goal, so only the difference is applied.
            state.timer = (state.timer + actual).saturating_sub(planned);
        }
    }
}

pub enum StandardMatchState {
    WaitingForGame(WarmupState),
    Game(Match),
}
pub struct WarmupState {
    pub(crate) time: u32,
    pub(crate) score: (u32, u32),
}
impl WarmupState {
    /// Ticks of warmup once both teams have players, before the game starts.
    const TIME: u32 = 2000;

    fn new() -> Self {
        Self {
            time: Self::TIME,
            score: (0, 0),
        }
    }
    fn before_tick(&mut self, red_players: usize, blue_players: usize) -> Option<(u32, u32)> {
        if red_players == 0 || blue_players == 0 {
            self.time = Self::TIME;
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
        if let StandardMatchState::Game(game) = &mut self.state
            && game.after_tick(ctx, &self.settings, events) == GameProgress::Finished
        {
            ctx.new_game();
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
            "sp" | "setposition" => {
                let requested = arg.to_ascii_uppercase();
                if let Some(position) = ALLOWED_POSITIONS
                    .iter()
                    .find(|position| **position == requested)
                {
                    self.settings.preferred_positions.insert(player, position);
                    ctx.messages.add_directed_server_chat_message(
                        format!("Preferred faceoff position set to {position}"),
                        player,
                    );
                } else {
                    ctx.messages
                        .add_directed_server_chat_message("Invalid faceoff position", player);
                }
            }
            "fs" => {
                if let Ok(index) = arg.parse::<PlayerIndex>() {
                    if ctx.require_admin(player).is_some() {
                        let target = ctx.players.get_by_index(index).map(|p| p.id);
                        if let Some(target) = target {
                            self.teams.force_spectate(ctx, target);
                        }
                    }
                }
            }
            _ => return CommandResult::Unknown,
        }
        CommandResult::Handled
    }
    fn game_started(&mut self, ctx: &mut GameContext) {
        self.state = StandardMatchState::WaitingForGame(WarmupState::new());
        // Players can leave without before_player_exit, such as bots removed by the game mode
        // or at the end of a game, so forget everyone who is gone.
        self.settings
            .preferred_positions
            .retain(|player_id, _| ctx.players.get(*player_id).is_some());
        let (width, length) = (ctx.rink.width, ctx.rink.length);
        let first_x =
            width / 2.0 - 0.4 * self.settings.config.warmup_pucks.saturating_sub(1) as f32;
        for index in 0..self.settings.config.warmup_pucks {
            ctx.objects.spawn_puck(Puck::new(
                Vec3::new(
                    first_x + 0.8 * index as f32,
                    self.settings.config.spawn_puck_altitude,
                    length / 2.0,
                ),
                Rot3::IDENTITY,
            ));
        }
    }
    fn before_player_exit(
        &mut self,
        _ctx: &mut GameContext,
        player: PlayerId,
        _reason: ExitReason,
    ) {
        self.teams.remove_player(player);
        self.settings.preferred_positions.remove(&player);
    }
    fn server_list_team_size(&self) -> u32 {
        self.teams.team_max as u32
    }
    fn include_tick_in_recording(&self) -> bool {
        matches!(self.state, StandardMatchState::Game(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::test_server::{EmptyGameMode, TestServer};

    fn player(index: usize) -> PlayerId {
        PlayerId {
            index: PlayerIndex(index),
            counter: 0,
        }
    }

    #[test]
    fn position_preferences_are_claimed_in_player_order() {
        let first = player(1);
        let second = player(2);
        let third = player(3);
        let preferred = HashMap::from([(first, "LW"), (second, "LW"), (third, "G")]);
        let assignments = assign_team_positions(&[first, second, third], &preferred);

        assert_eq!(assignments[&first], "LW");
        assert_eq!(assignments[&second], "C");
        assert_eq!(assignments[&third], "G");
    }

    #[test]
    fn a_team_without_a_center_gets_a_non_goalie_center() {
        let goalie = player(1);
        let skater = player(2);
        let preferred = HashMap::from([(goalie, "G"), (skater, "LW")]);
        let assignments = assign_team_positions(&[goalie, skater], &preferred);

        assert_eq!(assignments[&goalie], "G");
        assert_eq!(assignments[&skater], "C");
    }

    #[test]
    fn positions_are_assigned_independently_per_team() {
        let red = player(1);
        let blue = player(2);
        let preferred = HashMap::from([(red, "G"), (blue, "G")]);
        let mut assignments = assign_team_positions(&[red], &preferred);
        assignments.extend(assign_team_positions(&[blue], &preferred));

        assert_eq!(assignments[&red], "C");
        assert_eq!(assignments[&blue], "C");
    }

    #[test]
    fn center_stoppage_does_not_reset_the_clock() {
        let mut state = BreakState {
            timer: 0,
            kind: BreakKind::Stoppage(Stoppage {
                cause: StoppageCause::Goal,
                next_faceoff: Faceoff::Center,
            }),
        };

        assert!(matches!(
            state.before_tick(&MatchConfiguration::default(), false),
            MatchTransition::StartPlaying {
                faceoff: Faceoff::Center,
                reset_clock: false,
            }
        ));
    }

    #[test]
    fn intermission_resets_the_clock() {
        let mut state = BreakState {
            timer: 0,
            kind: BreakKind::Intermission {
                period_end_step: Some(0),
                too_late_printed: false,
            },
        };

        assert!(matches!(
            state.before_tick(&MatchConfiguration::default(), false),
            MatchTransition::StartPlaying {
                faceoff: Faceoff::Center,
                reset_clock: true,
            }
        ));
    }

    #[test]
    fn stoppage_after_a_deciding_goal_turns_into_game_over() {
        let config = MatchConfiguration::default();
        let mut state = BreakState {
            timer: 1,
            kind: BreakKind::Stoppage(Stoppage {
                cause: StoppageCause::Goal,
                next_faceoff: Faceoff::Center,
            }),
        };

        let transition = state.before_tick(&config, true);

        assert!(matches!(
            transition,
            MatchTransition::Break(BreakState {
                kind: BreakKind::GameOver,
                timer,
            }) if timer == config.time_intermission * 100
        ));
    }

    #[test]
    fn period_end_starts_an_intermission_or_ends_the_game() {
        let mut server = crate::server::test_server();
        let mut ctx = server.context();
        let config = MatchConfiguration::default();
        let settings = MatchSettings::new(config);

        let mut tied = Match::start(&config, (1, 1));
        assert_eq!(
            tied.apply(&mut ctx, &settings, MatchTransition::EndPeriod),
            GameProgress::Continue
        );
        assert_eq!(tied.game_state.period, 2);
        assert!(matches!(
            tied.state,
            MatchState::Break(BreakState {
                kind: BreakKind::Intermission {
                    period_end_step: Some(_),
                    ..
                },
                ..
            })
        ));

        let mut decided = Match::start(&config, (2, 1));
        decided.game_state.period = config.periods;
        decided.apply(&mut ctx, &settings, MatchTransition::EndPeriod);
        assert!(matches!(
            decided.state,
            MatchState::Break(BreakState {
                kind: BreakKind::GameOver,
                ..
            })
        ));
    }

    #[test]
    fn goal_pause_without_replay_is_the_break_time() {
        assert_eq!(goal_stoppage_time(10, None), 1000);
    }

    #[test]
    fn goal_pause_with_replay_keeps_the_break_time_and_a_pause_after_the_replay() {
        let longest_replay = REPLAY_BEFORE_GOAL + REPLAY_AFTER_GOAL + 1;
        for time_break in [0, 5, 10, 20] {
            for replay_length in [REPLAY_AFTER_GOAL + 1, 300, 500, longest_replay] {
                let timer = goal_stoppage_time(time_break, Some(replay_length));
                // The timer runs before and after the replay, but not during it.
                assert!(timer + replay_length >= time_break * 100);
                assert!(timer - REPLAY_AFTER_GOAL >= MIN_PAUSE_AFTER_REPLAY);
            }
        }

        // With the default 10 second break, a full replay leaves 3 seconds after it, and a
        // short replay leaves the rest of the break.
        assert_eq!(goal_stoppage_time(10, Some(longest_replay)), 500);
        assert_eq!(goal_stoppage_time(10, Some(300)), 700);
    }

    #[test]
    fn match_clocks_stand_still_during_replays() {
        let mut server = crate::server::test_server();
        server.state.replay.set_history_capacity(10);
        server.state.replay.advance_step();
        server
            .state
            .replay
            .record_tick(std::array::from_fn(|_| crate::protocol::ObjectPacket::None));
        let step = server.state.replay.game_step();
        let mut ctx = server.context();
        let config = MatchConfiguration::default();
        let settings = MatchSettings::new(config);
        let mut game = Match::start(&config, (0, 0));
        game.apply(
            &mut ctx,
            &settings,
            MatchTransition::StartPlaying {
                faceoff: Faceoff::Center,
                reset_clock: true,
            },
        );
        let period_time = game.game_state.time;

        game.before_tick(&mut ctx, &settings);
        assert_eq!(game.game_state.time, period_time - 1);

        ctx.replay.add_replay_to_queue(step, step, None);
        assert!(ctx.replay.is_in_replay());
        game.before_tick(&mut ctx, &settings);
        assert_eq!(game.game_state.time, period_time - 1);

        game.state = MatchState::Break(BreakState {
            timer: 5,
            kind: BreakKind::Stoppage(Stoppage {
                cause: StoppageCause::Goal,
                next_faceoff: Faceoff::Center,
            }),
        });
        game.before_tick(&mut ctx, &settings);
        assert!(matches!(
            game.state,
            MatchState::Break(BreakState { timer: 5, .. })
        ));
    }

    #[test]
    fn position_set_during_a_match_is_used_at_the_next_faceoff() {
        let mut server = crate::server::test_server();
        let player = server
            .state
            .player_message_state
            .add_player("goalie", "127.0.0.1:1".parse().unwrap())
            .unwrap();
        // A team needs a center, so the goalie needs a teammate to keep their position.
        let teammate = server
            .state
            .player_message_state
            .add_player("teammate", "127.0.0.1:2".parse().unwrap())
            .unwrap();
        let mut ctx = server.context();
        assert!(ctx.spawn_skater(player, Team::Red, Vec3::ZERO, Rot3::IDENTITY, false));
        assert!(ctx.spawn_skater(teammate, Team::Red, Vec3::X, Rot3::IDENTITY, false));
        let config = MatchConfiguration::default();
        let mut mode = StandardMatchGameMode::new(config, 5, SpawnPoint::Center);
        mode.state = StandardMatchState::Game(Match::start(&config, (0, 0)));

        mode.handle_command(&mut ctx, "sp", "g", player);
        let StandardMatchState::Game(game) = &mut mode.state else {
            panic!("expected a running match");
        };
        game.apply(
            &mut ctx,
            &mode.settings,
            MatchTransition::StartPlaying {
                faceoff: Faceoff::Center,
                reset_clock: true,
            },
        );

        let spot = faceoff_spot(
            ctx.rink,
            Faceoff::Center,
            config.spawn_point_offset,
            config.spawn_player_altitude,
        );
        assert_eq!(
            ctx.objects.skater(player).unwrap().body.pos,
            spot.red["G"].0
        );
    }

    #[test]
    fn finished_game_is_replaced_by_a_new_one_in_the_same_tick() {
        let config = MatchConfiguration {
            warmup_pucks: 3,
            ..MatchConfiguration::default()
        };
        let mut t = TestServer::new(StandardMatchGameMode::new(config, 5, SpawnPoint::Center));
        t.join("player");
        t.tick();
        let mut game = Match::start(&config, (2, 1));
        game.state = MatchState::Break(BreakState {
            timer: 1,
            kind: BreakKind::GameOver,
        });
        t.mode.state = StandardMatchState::Game(game);
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
            })
        ));
    }

    #[test]
    fn match_start_enters_an_opening_break() {
        let config = MatchConfiguration::default();
        let game = Match::start(&config, (0, 0));

        assert_eq!(game.game_state.period, 1);
        assert_eq!(game.game_state.time, 0);
        assert!(matches!(
            game.state,
            MatchState::Break(BreakState {
                kind: BreakKind::Intermission {
                    period_end_step: None,
                    ..
                },
                ..
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
        t.chat(player, "/reset");

        assert_eq!(
            t.take_chats(player),
            [
                "Preferred faceoff position set to LW",
                "Invalid faceoff position",
                "Unknown command /reset"
            ]
        );
    }

    #[test]
    fn goal_stoppage_follows_the_replay_ticks_that_were_queued() {
        let mut t = TestServer::new(EmptyGameMode);
        let player = t.join("player");
        let history = &mut t.server.state.replay;
        history.set_history_capacity(3);
        for _ in 0..5 {
            history.advance_step();
            history.record_tick(std::array::from_fn(|_| crate::protocol::ObjectPacket::None));
        }
        t.take_messages(player);
        let config = MatchConfiguration::default();
        let goal_stoppage = |timer| {
            let mut game = Match::start(&config, (0, 0));
            game.state = MatchState::Break(BreakState {
                timer,
                kind: BreakKind::Stoppage(Stoppage {
                    cause: StoppageCause::Goal,
                    next_faceoff: Faceoff::Center,
                }),
            });
            game
        };
        let timer = |game: &Match| match &game.state {
            MatchState::Break(state) => state.timer,
            MatchState::Playing(_) => panic!("expected a stoppage"),
        };

        // Steps 2 to 4 are in the history, so 2 of the 5 replay ticks are missing.
        let mut game = goal_stoppage(100);
        game.queue_goal_replay(&mut t.ctx(), &config, 0, 4, None);
        assert_eq!(timer(&game), 102);
        assert!(t.ctx().replay.is_in_replay());
        assert_eq!(t.take_chats(player), ["Goal replay"]);

        // None of these steps have been played, so there is no replay to announce.
        let mut game = goal_stoppage(100);
        game.queue_goal_replay(&mut t.ctx(), &config, 10, 12, None);
        assert_eq!(timer(&game), 103);
        assert_eq!(t.take_chats(player), Vec::<String>::new());

        // A full replay gets the minimum timer of 500 ticks instead of 1000 - 801. Without the
        // replay, the pause is the plain 10 second break again, not 500 ticks plus the replay.
        let longest_replay = REPLAY_BEFORE_GOAL + REPLAY_AFTER_GOAL + 1;
        assert_eq!(goal_stoppage_time(10, Some(longest_replay)), 500);
        let mut game = goal_stoppage(100);
        game.queue_goal_replay(&mut t.ctx(), &config, 10, 10 + longest_replay - 1, None);
        assert_eq!(timer(&game), 600);

        // With a 2 second break, the break is already over when the missing replay would start.
        let short_break = MatchConfiguration {
            time_break: 2,
            ..config
        };
        let mut game = goal_stoppage(500 - REPLAY_AFTER_GOAL);
        game.queue_goal_replay(
            &mut t.ctx(),
            &short_break,
            10,
            10 + longest_replay - 1,
            None,
        );
        assert_eq!(timer(&game), 0);
    }
}

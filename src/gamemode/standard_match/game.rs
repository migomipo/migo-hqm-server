//! A running match: periods, breaks, goals, offside, icing and faceoffs.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use glam::Vec3;
use glamx::Rot3;

use crate::game::RinkSideOfLine::{BlueSide, RedSide};
use crate::game::{PhysicsEvent, PlayerId, Puck, PuckId, RulesState, ScoreboardValues, Team};
use crate::gamemode::GameContext;
use crate::gamemode::util::GameProgress;
#[cfg(doc)]
use crate::gamemode::{GameMode, util::TeamSelection};
use crate::history::History;

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
///
/// A game mode running a [Match] with [MatchConfiguration::goal_replay] sets the replay history
/// to this length with [Replay::set_history_length](crate::gamemode::Replay::set_history_length).
pub const REPLAY_HISTORY_LENGTH: u32 = 1000;
const _: () = assert!(REPLAY_BEFORE_GOAL + REPLAY_AFTER_GOAL < REPLAY_HISTORY_LENGTH);
/// Touches remembered per puck for crediting goals and assists.
const PUCK_TOUCH_HISTORY_LEN: usize = 16;

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

impl IcingConfiguration {
    /// Describes the rule to players, as in /rules and the /set announcement.
    pub(super) fn description(self) -> &'static str {
        match self {
            Self::Off => "Icing disabled",
            Self::Touch => "Touch icing enabled",
            Self::NoTouch => "No-touch icing enabled",
        }
    }
}
impl OffsideConfiguration {
    /// Describes the rule to players, as in /rules and the /set announcement.
    pub(super) fn description(self) -> &'static str {
        match self {
            Self::Off => "Offside disabled",
            Self::Delayed => "Offside enabled",
            Self::Immediate => "Immediate offside enabled",
        }
    }
}
impl TwoLinePassConfiguration {
    /// Describes the rule to players, as in /rules and the /set announcement.
    pub(super) fn description(self) -> &'static str {
        match self {
            Self::Off => "Two-line pass rule disabled",
            Self::On => "Regular two-line pass rule enabled",
            Self::Forward => "Forward two-line pass rule enabled",
            Self::Double => "Regular and forward two-line pass rules enabled",
            Self::ThreeLine => "Three-line pass rule enabled",
        }
    }
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
            spawn_player_altitude: 1.5,
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
    /// Whether an admin has paused the game. The timer stands still while paused.
    paused: bool,
}
#[derive(Debug, Clone)]
enum BreakKind {
    Stoppage(Stoppage),
    Intermission {
        period_end_step: Option<u32>,
        too_late_printed: bool,
    },
    GameOver {
        /// Whether the game was decided by a goal, whose message is shown during the break.
        after_goal: bool,
    },
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
    /// An admin stopped play with /faceoff or /pause.
    Admin,
}
/// A change to the state of a running match.
///
/// The current state decides which transition to make, and [Match::apply] carries it out. A match
/// only ends once its game-over break has run out, which [BreakState::before_tick] reports with
/// [BreakOutcome::GameFinished] instead of a transition.
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
}
/// What happens when a break's timer is counted down.
#[derive(Debug)]
enum BreakOutcome {
    Transition(MatchTransition),
    /// The game-over break has run out and the game is over.
    GameFinished,
}
#[derive(Debug)]
struct PlayingState {
    icing: Icing,
    offside: Offside,
    twoline: TwoLine,
    pass: Option<Pass>,
    started_goalies: HashSet<PlayerId>,
    puck_touches: HashMap<PuckId, History<PuckTouch>>,
    faceoff_step: u32,
}
impl BreakState {
    fn new(timer: u32, kind: BreakKind) -> Self {
        Self {
            timer,
            kind,
            paused: false,
        }
    }
    fn before_tick(&mut self, config: &MatchConfiguration, game_over: bool) -> BreakOutcome {
        if self.paused {
            return BreakOutcome::Transition(MatchTransition::Continue);
        }
        self.timer = self.timer.saturating_sub(1);
        if self.timer != 0 {
            return BreakOutcome::Transition(MatchTransition::Continue);
        }
        let transition = match &self.kind {
            BreakKind::Stoppage(_) if game_over => MatchTransition::Break(BreakState::new(
                config.time_intermission * 100,
                BreakKind::GameOver { after_goal: false },
            )),
            BreakKind::Stoppage(stoppage) => MatchTransition::StartPlaying {
                faceoff: stoppage.next_faceoff,
                reset_clock: false,
            },
            BreakKind::Intermission { .. } => MatchTransition::StartPlaying {
                faceoff: Faceoff::Center,
                reset_clock: true,
            },
            BreakKind::GameOver { .. } => return BreakOutcome::GameFinished,
        };
        BreakOutcome::Transition(transition)
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
            BreakKind::Stoppage(Stoppage {
                cause: StoppageCause::Admin,
                ..
            }) => (RulesState::default(), 0, false),
            BreakKind::Intermission { .. } => (RulesState::default(), 0, false),
            BreakKind::GameOver { after_goal } => (
                RulesState::default(),
                if after_goal { self.timer } else { 0 },
                true,
            ),
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
/// Something that happened in a [Match] during a tick, returned from [Match::after_tick].
///
/// These are the events a game mode built on a match may want to react to, such as for keeping
/// statistics. The match announces goals and calls in chat itself. Events of a tick are in the
/// order they happened, so a deciding goal is followed by [MatchEvent::GameOver].
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum MatchEvent {
    /// A goal was scored for `team`. A goal scored while the scoring team is offside is called
    /// as [MatchEvent::Offside] instead.
    Goal {
        team: Team,
        /// The last player of the scoring team to touch the puck, if any.
        scorer: Option<PlayerId>,
        assist: Option<PlayerId>,
        /// Speed of the puck as it crossed the goal line, in meters per tick.
        speed_across_line: f32,
        /// Speed of the puck as it left the scorer's stick, in meters per tick.
        speed_from_stick: Option<f32>,
    },
    /// Offside was called on `team`, and play was stopped.
    Offside { team: Team },
    /// Icing was called on `team`, and play was stopped.
    Icing { team: Team },
    /// A two-line pass was called on `team`, and play was stopped.
    TwoLinePass { team: Team },
    /// The clock ran out in `period`. An intermission or the end of the game follows.
    PeriodEnded { period: u32 },
    /// The game has been decided, and the game-over break has begun. The match is finished once
    /// [Match::before_tick] returns [GameProgress::Finished].
    GameOver {
        /// The team with the most goals, or `None` if an admin has set the score to a tie.
        winner: Option<Team>,
    },
}

/// A running match: periods, intermissions, goals, and offside and icing calls.
///
/// [StandardMatchGameMode](super::StandardMatchGameMode) runs one after warmup, and a custom game mode can run one with its own
/// rules for joining teams and starting games. Such a game mode calls the methods of the match
/// from its own [GameMode] hooks:
///
/// - [Match::start] when its game should start. The match opens with an intermission, and puts
///   the players on teams at their faceoff positions when it is over.
/// - [Match::before_tick] in [GameMode::before_tick], after players have joined teams. Once it
///   returns [GameProgress::Finished], the game mode decides what comes next, such as calling
///   [GameContext::new_game].
/// - [Match::after_tick] and [Match::scoreboard] in [GameMode::after_tick].
/// - [Match::player_left_ice] whenever the game mode takes a player off the ice, such as the
///   players that [TeamSelection::update] and [TeamSelection::handle_force_spectate_command]
///   report. Players who leave the server do not need to be reported, as a player who comes back
///   gets a new [PlayerId].
///
/// For goal replays, the game mode also sets the replay history to [REPLAY_HISTORY_LENGTH] ticks
/// in [GameMode::init]. The match does not touch players who are not on a team, or pucks outside
/// faceoffs. [MatchSettings] holds what lasts across matches, such as players' preferred
/// positions, and admins can stop and resume play with [Match::pause], [Match::unpause] and
/// [Match::call_faceoff].
pub struct Match {
    state: MatchState,
    pub(crate) game_state: MatchGameState,
    /// Events from [Match::before_tick] that [Match::after_tick] returns with its own.
    pending_events: Vec<MatchEvent>,
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

    /// Sets the faceoff position a player would like to take, such as "C" or "G", ignoring
    /// case. Returns the position, or `None` if it is not one of [ALLOWED_POSITIONS].
    pub fn set_preferred_position(
        &mut self,
        player: PlayerId,
        position: &str,
    ) -> Option<&'static str> {
        let position = ALLOWED_POSITIONS
            .into_iter()
            .find(|allowed| allowed.eq_ignore_ascii_case(position))?;
        self.preferred_positions.insert(player, position);
        Some(position)
    }

    /// Forgets a player's preferred position. Call this when a player leaves the server.
    pub fn remove_player(&mut self, player: PlayerId) {
        self.preferred_positions.remove(&player);
    }

    /// Forgets the preferred positions of everyone who is no longer on the server.
    ///
    /// Players can leave without [GameMode::before_player_exit], such as bots removed by the game
    /// mode or at the end of a game, so call this when a new game starts.
    pub fn forget_departed_players(&mut self, ctx: &GameContext) {
        self.preferred_positions
            .retain(|player_id, _| ctx.players.get(*player_id).is_some());
    }
}

impl Match {
    /// Starts a match with the given score, with an intermission before the opening faceoff.
    pub fn start(config: &MatchConfiguration, score: (u32, u32)) -> Self {
        Self {
            state: MatchState::Break(BreakState::new(
                config.time_intermission * 100,
                BreakKind::Intermission {
                    period_end_step: None,
                    too_late_printed: false,
                },
            )),
            game_state: MatchGameState {
                time: 0,
                score,
                period: 1,
                replay: None,
            },
            pending_events: Vec::new(),
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
        // The puck starts inside the zone at a zone faceoff, so the attacking team is already in
        // it. Without this, the puck touching the blue line and coming back in would count as
        // entering the zone, and could be called offside.
        let offside = if rink.blue_zone_blue_line.side_of_line(puck_pos, 0.0) == BlueSide {
            Offside::InZone(Team::Red)
        } else if rink.red_zone_blue_line.side_of_line(puck_pos, 0.0) == RedSide {
            Offside::InZone(Team::Blue)
        } else {
            Offside::Neutral
        };
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
            offside,
            twoline: TwoLine::No,
            pass: None,
            started_goalies,
            puck_touches: HashMap::new(),
            faceoff_step: ctx.replay.game_step(),
        }
    }
}

/// Seconds of pause after a goal, until the next faceoff or, for a goal that decides the game,
/// until the next game.
///
/// A deciding goal goes straight to game over, so its one pause lasts as long as the longer of
/// the break and the intermission.
fn goal_pause(config: &MatchConfiguration, game_over: bool) -> u32 {
    if game_over {
        config.time_break.max(config.time_intermission)
    } else {
        config.time_break
    }
}

/// Ticks of break timer after a goal, for a pause of `pause` seconds.
///
/// The break timer stands still while a replay is shown, so with a replay the pause after the
/// goal is the timer plus the replay length. The timer is chosen so that the pause is at least
/// `pause` seconds, and so that at least [MIN_PAUSE_AFTER_REPLAY] ticks are left after the
/// replay, which starts [REPLAY_AFTER_GOAL] ticks after the goal.
fn goal_stoppage_time(pause: u32, replay_length: Option<u32>) -> u32 {
    let pause_time = pause * 100;
    match replay_length {
        None => pause_time,
        Some(replay_length) => pause_time
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
        // Red attacks towards the blue end of the rink, and Blue towards the red end.
        let past_side = match team {
            Team::Red => BlueSide,
            Team::Blue => RedSide,
        };
        ctx.objects
            .skaters()
            .filter(|&(id, skater)| {
                Some(id) != ignore
                    && ctx.players.get(id).and_then(|p| p.team()) == Some(team)
                    && line.side_of_line(skater.feet_position(), 0.0) == past_side
            })
            .map(|(id, _)| id)
            .collect()
    }
    /// Whether a player of `team` is past the offside line.
    fn team_in_zone(
        config: &MatchConfiguration,
        ctx: &GameContext,
        team: Team,
        ignore: Option<PlayerId>,
    ) -> bool {
        let center = config.offside_line == OffsideLineConfiguration::Center;
        !Self::players_past(ctx, team, ignore, center).is_empty()
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
        BreakState::new(
            timer,
            BreakKind::Stoppage(Stoppage {
                cause,
                next_faceoff,
            }),
        )
    }
    fn call_offside(
        config: &MatchConfiguration,
        ctx: &mut GameContext,
        team: Team,
        side: RinkSide,
        from: Option<PassLocation>,
        match_events: &mut Vec<MatchEvent>,
    ) -> BreakState {
        // IIHF rule 83.6: a pass is faced off in the zone it came from, at the spot with the least
        // territorial advantage, and a puck carried in is faced off at the spot just outside the
        // offside line. A carry's last touch is right before the line, so `from` tells the two
        // apart, whoever touches the puck next. Outside the center line is the offending team's
        // own half.
        let next_faceoff = if from.is_some_and(|p| p <= PassLocation::ReachedOwnBlue) {
            Faceoff::Defensive(team, side)
        } else if from.is_some_and(|p| p <= PassLocation::ReachedCenter)
            || config.offside_line == OffsideLineConfiguration::Center
        {
            Faceoff::Offside(team, side)
        } else {
            Faceoff::Offside(team.get_other_team(), side)
        };
        ctx.messages.add_server_chat_message("Offside");
        match_events.push(MatchEvent::Offside { team });
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
        match_events: &mut Vec<MatchEvent>,
    ) -> BreakState {
        let next_faceoff = Faceoff::Defensive(team, side);
        ctx.messages.add_server_chat_message("Icing");
        match_events.push(MatchEvent::Icing { team });
        Self::stoppage(config.time_break * 100, StoppageCause::Icing, next_faceoff)
    }
    fn call_twoline(
        config: &MatchConfiguration,
        ctx: &mut GameContext,
        team: Team,
        side: RinkSide,
        from: PassLocation,
        match_events: &mut Vec<MatchEvent>,
    ) -> BreakState {
        // Two-line passes are only called on passes from the passer's own half, so like an
        // offside pass, it is faced off in the zone the pass came from, at the spot with the
        // least territorial advantage.
        let next_faceoff = if from <= PassLocation::ReachedOwnBlue {
            Faceoff::Defensive(team, side)
        } else {
            Faceoff::Offside(team, side)
        };
        ctx.messages.add_server_chat_message("Two-line pass");
        match_events.push(MatchEvent::TwoLinePass { team });
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
        match_events: &mut Vec<MatchEvent>,
    ) -> BreakState {
        if let Offside::Warning(warning) = playing.offside
            && warning.offending_team == team
        {
            return Self::call_offside(
                config,
                ctx,
                team,
                warning.side,
                warning.pass_origin,
                match_events,
            );
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
                .and_then(|touches| touches.newest().map(|touch| touch.player));
            if let Some(touches) = playing.puck_touches.get(&puck) {
                for touch in touches.iter_newest_first() {
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
        match_events.push(MatchEvent::Goal {
            team,
            scorer: goal_scorer,
            assist,
            speed_across_line: puck_speed_across_line,
            speed_from_stick: puck_speed_from_stick,
        });
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
        let game_over = game_state.game_over(config);
        let timer = goal_stoppage_time(goal_pause(config, game_over), replay_length);
        if game_over {
            // The game over shows right away, along with the goal message.
            BreakState::new(timer, BreakKind::GameOver { after_goal: true })
        } else {
            Self::stoppage(timer, StoppageCause::Goal, Faceoff::Center)
        }
    }
    fn touch(
        game_state: &MatchGameState,
        config: &MatchConfiguration,
        ctx: &mut GameContext,
        playing: &mut PlayingState,
        player: PlayerId,
        puck: PuckId,
        match_events: &mut Vec<MatchEvent>,
    ) -> Option<BreakState> {
        let team = ctx.players.get(player).and_then(|p| p.team())?;
        let side = Self::side(ctx, puck);
        if let Some(puck_object) = ctx.objects.get_puck(puck) {
            let touches = playing
                .puck_touches
                .entry(puck)
                .or_insert_with(|| History::new(PUCK_TOUCH_HISTORY_LEN));
            if let Some(last_touch) = touches
                .newest_mut()
                .filter(|touch| touch.player == player && touch.team == team)
            {
                last_touch.last_time = game_state.time;
                last_touch.puck_speed = puck_object.body.linear_velocity.length();
            } else {
                touches.push(PuckTouch {
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
        if let Offside::Warning(warning) = playing.offside
            && warning.offending_team == team
        {
            return Some(Self::call_offside(
                config,
                ctx,
                team,
                warning.side,
                warning.pass_origin,
                match_events,
            ));
        }
        if let TwoLine::Warning(warning) = playing.twoline.clone() {
            if warning.passing_team == team && warning.twoline_offside_players.contains(&player) {
                return Some(Self::call_twoline(
                    config,
                    ctx,
                    team,
                    warning.side,
                    warning.pass_origin,
                    match_events,
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
                    match_events,
                ));
            } else {
                playing.icing = Icing::No;
                ctx.messages.add_server_chat_message("Icing waved off");
            }
        }
        None
    }
    fn update_pass(playing: &mut PlayingState, team: Team, at: PassLocation) {
        if let Some(pass) = &mut playing.pass
            && pass.team == team
            && pass.from.is_none()
        {
            pass.from = Some(at);
        }
    }
    fn wave_off_twoline(ctx: &mut GameContext, playing: &mut PlayingState, team: Team) {
        if let TwoLine::Warning(warning) = &playing.twoline
            && warning.passing_team != team
        {
            playing.twoline = TwoLine::No;
            ctx.messages
                .add_server_chat_message("Two-line pass waved off");
        }
    }
    fn enter_zone(
        config: &MatchConfiguration,
        ctx: &mut GameContext,
        playing: &mut PlayingState,
        team: Team,
        blue_line: bool,
        match_events: &mut Vec<MatchEvent>,
    ) -> Option<BreakState> {
        let enabled = (blue_line && config.offside_line == OffsideLineConfiguration::OffensiveBlue)
            || (!blue_line && config.offside_line == OffsideLineConfiguration::Center);
        if enabled && !matches!(playing.offside, Offside::InZone(t) if t == team) {
            if let Some(pass) = playing.pass {
                if pass.team == team && Self::team_in_zone(config, ctx, team, Some(pass.player)) {
                    match config.offside {
                        OffsideConfiguration::Delayed => {
                            playing.offside = Offside::Warning(OffsideWarning {
                                offending_team: team,
                                side: pass.side,
                                pass_origin: pass.from,
                            });
                            ctx.messages.add_server_chat_message("Offside warning");
                        }
                        OffsideConfiguration::Immediate => {
                            return Some(Self::call_offside(
                                config,
                                ctx,
                                team,
                                pass.side,
                                pass.from,
                                match_events,
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
        // A two-line pass warning stays as it is until it is called or waved off, so a pass that
        // crosses two lines keeps the players that were offside at the first one.
        if let Some(pass) = playing.pass
            && matches!(playing.twoline, TwoLine::No)
            && pass.team == team
            && Self::twoline_is_active(config, blue_line, pass.from)
        {
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
        match_events: &mut Vec<MatchEvent>,
    ) -> MatchTransition {
        for event in events {
            match *event {
                PhysicsEvent::PuckTouch { player, puck } => {
                    if let Some(stoppage) =
                        Self::touch(game_state, config, ctx, playing, player, puck, match_events)
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
                        match_events,
                    ));
                }
                PhysicsEvent::PuckPassedGoalLine { team, puck: _ } => {
                    if let Some(pass) = playing.pass
                        && pass.team == team.get_other_team()
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
                                    config,
                                    ctx,
                                    pass.team,
                                    pass.side,
                                    match_events,
                                ));
                            }
                            IcingConfiguration::Off => {}
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
                        if let Offside::Warning(warning) = playing.offside
                            && team.get_other_team() == warning.offending_team
                        {
                            ctx.messages.add_server_chat_message("Offside waved off");
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
                    if let Some(stoppage) =
                        Self::enter_zone(config, ctx, playing, team, false, match_events)
                    {
                        return MatchTransition::Break(stoppage);
                    }
                    if let Offside::Warning(warning) = playing.offside
                        && warning.offending_team != team
                    {
                        ctx.messages.add_server_chat_message("Offside waved off");
                    }
                }
                PhysicsEvent::PuckReachedOffensiveZone { team, .. } => {
                    Self::update_pass(playing, team, PassLocation::ReachedOffensive)
                }
                PhysicsEvent::PuckEnteredOffensiveZone { team, .. } => {
                    Self::update_pass(playing, team, PassLocation::PassedOffensive);
                    if let Some(stoppage) =
                        Self::enter_zone(config, ctx, playing, team, true, match_events)
                    {
                        return MatchTransition::Break(stoppage);
                    }
                }
                _ => {}
            }
        }
        if let Offside::Warning(warning) = playing.offside
            && !Self::team_in_zone(config, ctx, warning.offending_team, None)
        {
            playing.offside = Offside::InZone(warning.offending_team);
            ctx.messages.add_server_chat_message("Offside waved off");
        }
        MatchTransition::Continue
    }
}

impl Match {
    /// Tells the match that a player's skater has left the ice. A goalie who leaves the ice is
    /// treated as a skater from then on, until the next faceoff, even if they are already back on
    /// the ice.
    pub fn player_left_ice(&mut self, player: PlayerId) {
        if let MatchState::Playing(playing) = &mut self.state {
            playing.started_goalies.remove(&player);
        }
    }
    /// Runs the period clock and the break timers, and sets up faceoffs when breaks end.
    ///
    /// Returns [GameProgress::Finished] once the game-over break has run out. Events from this
    /// call, such as [MatchEvent::PeriodEnded], are returned from the next [Match::after_tick].
    pub fn before_tick(&mut self, ctx: &mut GameContext, settings: &MatchSettings) -> GameProgress {
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
            MatchState::Break(state) => match state.before_tick(
                &settings.config,
                self.game_state.game_over(&settings.config),
            ) {
                BreakOutcome::Transition(transition) => transition,
                BreakOutcome::GameFinished => return GameProgress::Finished,
            },
        };
        self.apply(ctx, settings, transition);
        GameProgress::Continue
    }
    /// Carries out a transition. This is the only place where a running match changes state.
    fn apply(
        &mut self,
        ctx: &mut GameContext,
        settings: &MatchSettings,
        transition: MatchTransition,
    ) {
        match transition {
            MatchTransition::Continue => {}
            MatchTransition::Break(state) => self.enter_break(state),
            MatchTransition::EndPeriod => {
                self.pending_events.push(MatchEvent::PeriodEnded {
                    period: self.game_state.period,
                });
                self.game_state.period += 1;
                self.enter_break(BreakState::new(
                    settings.config.time_intermission * 100,
                    if self.game_state.game_over(&settings.config) {
                        BreakKind::GameOver { after_goal: false }
                    } else {
                        BreakKind::Intermission {
                            period_end_step: Some(ctx.replay.game_step()),
                            too_late_printed: false,
                        }
                    },
                ));
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
        }
    }
    fn enter_break(&mut self, state: BreakState) {
        let was_over = matches!(
            self.state,
            MatchState::Break(BreakState {
                kind: BreakKind::GameOver { .. },
                ..
            })
        );
        if !was_over && matches!(state.kind, BreakKind::GameOver { .. }) {
            let (red, blue) = self.game_state.score;
            let winner = match red.cmp(&blue) {
                Ordering::Greater => Some(Team::Red),
                Ordering::Less => Some(Team::Blue),
                Ordering::Equal => None,
            };
            self.pending_events.push(MatchEvent::GameOver { winner });
        }
        self.state = MatchState::Break(state);
    }
    /// The score as (red, blue).
    pub fn score(&self) -> (u32, u32) {
        self.game_state.score
    }
    /// The current period, starting from 1. Periods past [MatchConfiguration::periods] are
    /// overtime.
    pub fn period(&self) -> u32 {
        self.game_state.period
    }
    /// Ticks left on the period clock.
    pub fn time(&self) -> u32 {
        self.game_state.time
    }
    /// Whether play is on, rather than stopped for a break.
    pub fn is_playing(&self) -> bool {
        matches!(self.state, MatchState::Playing(_))
    }
    /// Whether the game has been decided, and the game-over break is on.
    pub fn is_game_over(&self) -> bool {
        matches!(
            self.state,
            MatchState::Break(BreakState {
                kind: BreakKind::GameOver { .. },
                ..
            })
        )
    }
    /// The values to show on the scoreboard, to return from [GameMode::after_tick].
    pub fn scoreboard(&self) -> ScoreboardValues {
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
    /// Handles the physics events of a tick, and returns what happened in the match during the
    /// tick, including in [Match::before_tick].
    pub fn after_tick(
        &mut self,
        ctx: &mut GameContext,
        settings: &MatchSettings,
        events: &[PhysicsEvent],
    ) -> Vec<MatchEvent> {
        let mut match_events = std::mem::take(&mut self.pending_events);
        let transition = match &mut self.state {
            MatchState::Playing(playing) => PlayingState::events(
                &mut self.game_state,
                &settings.config,
                ctx,
                events,
                playing,
                &mut match_events,
            ),
            MatchState::Break(state) => {
                state.handle_events(ctx, events);
                MatchTransition::Continue
            }
        };
        self.apply(ctx, settings, transition);
        if let Some((start_step, end_step, force_view)) = self.game_state.replay
            && end_step <= ctx.replay.game_step()
        {
            self.game_state.replay = None;
            self.queue_goal_replay(ctx, &settings.config, start_step, end_step, force_view);
        }
        match_events.append(&mut self.pending_events);
        match_events
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
        if let MatchState::Break(state) = &mut self.state {
            let pause = goal_pause(
                config,
                matches!(state.kind, BreakKind::GameOver { after_goal: true }),
            );
            let planned = goal_stoppage_time(pause, Some(end_step - start_step + 1));
            let actual = goal_stoppage_time(pause, (queued > 0).then_some(queued));
            // The timer has been running since the goal, so only the difference is applied.
            state.timer = (state.timer + actual).saturating_sub(planned);
        }
    }
}

/// Ticks from /faceoff until the faceoff.
pub const FACEOFF_COMMAND_DELAY: u32 = 500;

/// Admin controls, as used by /faceoff, /pause and /unpause. Each returns why it could not be
/// done, as a message for the admin.
impl Match {
    /// Calls a center-ice faceoff in [FACEOFF_COMMAND_DELAY] ticks, and unpauses the game.
    pub fn call_faceoff(
        &mut self,
        ctx: &mut GameContext,
        settings: &MatchSettings,
    ) -> Result<(), &'static str> {
        match &mut self.state {
            MatchState::Break(BreakState {
                kind: BreakKind::GameOver { .. },
                ..
            }) => return Err("The game is over"),
            // The intermission is shortened rather than replaced, as the clock is reset when it
            // ends.
            MatchState::Break(
                state @ BreakState {
                    kind: BreakKind::Intermission { .. },
                    ..
                },
            ) => {
                state.timer = FACEOFF_COMMAND_DELAY;
                state.paused = false;
            }
            _ => {
                // A goal replay that has not been queued yet would hold up the faceoff.
                self.game_state.replay = None;
                let stoppage = Self::admin_stoppage(FACEOFF_COMMAND_DELAY);
                self.apply(ctx, settings, MatchTransition::Break(stoppage));
            }
        }
        Ok(())
    }
    /// Holds the break timer, or stops play if it is on. Play resumes with a center-ice faceoff.
    pub fn pause(
        &mut self,
        ctx: &mut GameContext,
        settings: &MatchSettings,
    ) -> Result<(), &'static str> {
        match &mut self.state {
            MatchState::Break(state) if state.paused => return Err("The game is already paused"),
            MatchState::Break(state) => state.paused = true,
            MatchState::Playing(_) => {
                let mut stoppage = Self::admin_stoppage(settings.config.time_break * 100);
                stoppage.paused = true;
                self.apply(ctx, settings, MatchTransition::Break(stoppage));
            }
        }
        Ok(())
    }
    /// Lets the break timer run again, with at least a full break left so that players are not
    /// caught by the faceoff.
    pub fn unpause(
        &mut self,
        _ctx: &mut GameContext,
        settings: &MatchSettings,
    ) -> Result<(), &'static str> {
        match &mut self.state {
            MatchState::Break(state) if state.paused => {
                state.paused = false;
                state.timer = state.timer.max(settings.config.time_break * 100);
                Ok(())
            }
            _ => Err("The game is not paused"),
        }
    }
    fn admin_stoppage(timer: u32) -> BreakState {
        BreakState::new(
            timer,
            BreakKind::Stoppage(Stoppage {
                cause: StoppageCause::Admin,
                next_faceoff: Faceoff::Center,
            }),
        )
    }
}

#[cfg(test)]
impl Match {
    /// A match in its game-over break, which runs out in `timer` ticks.
    pub(super) fn in_game_over_break(
        config: &MatchConfiguration,
        score: (u32, u32),
        timer: u32,
    ) -> Self {
        let mut game = Self::start(config, score);
        game.state = MatchState::Break(BreakState::new(
            timer,
            BreakKind::GameOver { after_goal: false },
        ));
        game
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::PlayerIndex;
    use crate::gamemode::GameMode;
    use crate::gamemode::standard_match::test_game::{
        MatchGame, PUCK_LANE, RECEIVER_LANE, ice_the_puck, red_attack,
    };
    use crate::gamemode::standard_match::{StandardMatchGameMode, StandardMatchState};
    use crate::gamemode::util::SpawnPoint;
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
        let mut state = BreakState::new(
            0,
            BreakKind::Stoppage(Stoppage {
                cause: StoppageCause::Goal,
                next_faceoff: Faceoff::Center,
            }),
        );

        assert!(matches!(
            state.before_tick(&MatchConfiguration::default(), false),
            BreakOutcome::Transition(MatchTransition::StartPlaying {
                faceoff: Faceoff::Center,
                reset_clock: false,
            })
        ));
    }

    #[test]
    fn intermission_resets_the_clock() {
        let mut state = BreakState::new(
            0,
            BreakKind::Intermission {
                period_end_step: Some(0),
                too_late_printed: false,
            },
        );

        assert!(matches!(
            state.before_tick(&MatchConfiguration::default(), false),
            BreakOutcome::Transition(MatchTransition::StartPlaying {
                faceoff: Faceoff::Center,
                reset_clock: true,
            })
        ));
    }

    #[test]
    fn stoppage_ending_with_the_game_decided_turns_into_game_over() {
        // Deciding goals go to game over right away, but an admin can set a deciding score
        // during any stoppage.
        let config = MatchConfiguration::default();
        let mut state = BreakState::new(
            1,
            BreakKind::Stoppage(Stoppage {
                cause: StoppageCause::Offside,
                next_faceoff: Faceoff::Center,
            }),
        );

        let outcome = state.before_tick(&config, true);

        assert!(matches!(
            outcome,
            BreakOutcome::Transition(MatchTransition::Break(BreakState {
                kind: BreakKind::GameOver { after_goal: false },
                timer,
                ..
            })) if timer == config.time_intermission * 100
        ));
    }

    #[test]
    fn paused_break_holds_its_timer_even_when_the_game_is_decided() {
        let mut state = BreakState::new(
            1,
            BreakKind::Stoppage(Stoppage {
                cause: StoppageCause::Admin,
                next_faceoff: Faceoff::Center,
            }),
        );
        state.paused = true;

        assert!(matches!(
            state.before_tick(&MatchConfiguration::default(), true),
            BreakOutcome::Transition(MatchTransition::Continue)
        ));
        assert_eq!(state.timer, 1);
    }

    #[test]
    fn period_end_starts_an_intermission_or_ends_the_game() {
        let mut server = crate::server::test_server();
        let mut ctx = server.context();
        let config = MatchConfiguration::default();
        let settings = MatchSettings::new(config);

        let mut tied = Match::start(&config, (1, 1));
        tied.apply(&mut ctx, &settings, MatchTransition::EndPeriod);
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
                kind: BreakKind::GameOver { after_goal: false },
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
        server.state.replay.record_tick(
            std::array::from_fn(|_| crate::protocol::ObjectPacket::None),
            std::array::from_fn(|_| None),
        );
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

        game.state = MatchState::Break(BreakState::new(
            5,
            BreakKind::Stoppage(Stoppage {
                cause: StoppageCause::Goal,
                next_faceoff: Faceoff::Center,
            }),
        ));
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

    fn add_player(server: &mut crate::server::HQMServer, name: &str, port: u16) -> PlayerId {
        server
            .state
            .player_message_state
            .add_player(name, ([127, 0, 0, 1], port).into())
            .unwrap()
    }

    #[test]
    fn players_count_as_past_a_line_at_the_same_distance_for_both_teams() {
        let mut server = crate::server::test_server();
        let red = add_player(&mut server, "red", 1);
        let blue = add_player(&mut server, "blue", 2);
        let mut ctx = server.context();
        let red_line_z = ctx.rink.blue_zone_blue_line.z;
        let blue_line_z = ctx.rink.red_zone_blue_line.z;
        let half_width = ctx.rink.blue_zone_blue_line.width / 2.0;

        for offset in [half_width - 0.01, half_width + 0.01] {
            // Each skater is the same distance past the middle of the line into the zone they
            // attack: Red towards low z, Blue towards high z.
            let at = |z| Vec3::new(15.0, 1.5, z);
            assert!(ctx.spawn_skater(
                red,
                Team::Red,
                at(red_line_z - offset),
                Rot3::IDENTITY,
                false
            ));
            assert!(ctx.spawn_skater(
                blue,
                Team::Blue,
                at(blue_line_z + offset),
                Rot3::IDENTITY,
                false
            ));

            let past = offset > half_width;
            assert_eq!(
                PlayingState::team_in_zone(&MatchConfiguration::default(), &ctx, Team::Red, None),
                past,
                "{offset}"
            );
            assert_eq!(
                PlayingState::team_in_zone(&MatchConfiguration::default(), &ctx, Team::Blue, None),
                past,
                "{offset}"
            );
        }
    }

    #[test]
    fn zone_faceoffs_count_as_already_inside_the_zone() {
        let mut server = crate::server::test_server();
        let passer = add_player(&mut server, "passer", 1);
        let teammate = add_player(&mut server, "teammate", 2);
        let mut ctx = server.context();
        for player in [passer, teammate] {
            assert!(ctx.spawn_skater(player, Team::Red, Vec3::ZERO, Rot3::IDENTITY, false));
        }
        let config = MatchConfiguration {
            offside: OffsideConfiguration::Delayed,
            ..MatchConfiguration::default()
        };
        let positions = HashMap::new();
        let offside_after = |ctx: &mut GameContext, faceoff| {
            Match::setup_faceoff(ctx, &config, &positions, faceoff).offside
        };

        assert!(matches!(
            offside_after(&mut ctx, Faceoff::Center),
            Offside::Neutral
        ));
        assert!(matches!(
            offside_after(&mut ctx, Faceoff::Offside(Team::Red, RinkSide::Lower)),
            Offside::Neutral
        ));
        assert!(matches!(
            offside_after(&mut ctx, Faceoff::Defensive(Team::Red, RinkSide::Higher)),
            Offside::InZone(Team::Blue)
        ));

        // A faceoff in Blue's zone. The puck touching the blue line and coming back in is not
        // offside, even though Red's players are in the zone.
        let mut playing = Match::setup_faceoff(
            &mut ctx,
            &config,
            &positions,
            Faceoff::Defensive(Team::Blue, RinkSide::Lower),
        );
        assert!(matches!(playing.offside, Offside::InZone(Team::Red)));
        let (puck, _) = ctx.objects.pucks().next().unwrap();
        playing.pass = Some(Pass {
            team: Team::Red,
            side: RinkSide::Lower,
            from: None,
            player: passer,
        });
        let mut game_state = Match::start(&config, (0, 0)).game_state;
        let transition = PlayingState::events(
            &mut game_state,
            &config,
            &mut ctx,
            &[PhysicsEvent::PuckEnteredOffensiveZone {
                team: Team::Red,
                puck,
            }],
            &mut playing,
            &mut Vec::new(),
        );
        assert!(matches!(transition, MatchTransition::Continue));
        assert!(matches!(playing.offside, Offside::InZone(Team::Red)));
    }

    /// A match with a red goalie, `red[1]`, with play on after the opening faceoff.
    fn match_with_goalie() -> MatchGame {
        let mut game = MatchGame::join(MatchGame::quick_config(), 2, 1);
        game.chat(game.red[1], "/sp g");
        game.play_until_faceoff();
        assert!(is_goalie(&game, game.red[1]));
        game
    }

    fn is_goalie(game: &MatchGame, player: PlayerId) -> bool {
        match &game.t.mode.mode.state {
            StandardMatchState::Game(Match {
                state: MatchState::Playing(playing),
                ..
            }) => playing.started_goalies.contains(&player),
            _ => panic!("expected play to be on"),
        }
    }

    #[test]
    fn goalie_who_leaves_the_ice_is_not_the_goalie_on_return() {
        let mut game = match_with_goalie();
        let goalie = game.red[1];

        // Without a team switch cooldown, the goalie is back on the next tick.
        game.t.mode.mode.teams.team_switch_cooldown = 0;
        game.chat(game.admin(), &format!("/fs {}", goalie.index.0));
        game.t.press_join(goalie, Team::Red);
        game.tick();
        assert!(game.t.ctx().objects.skater(goalie).is_some());
        assert!(!is_goalie(&game, goalie));
    }

    #[test]
    fn goalie_who_spectates_is_not_the_goalie_on_return() {
        let mut game = match_with_goalie();
        let goalie = game.red[1];
        game.t.mode.mode.teams.team_switch_cooldown = 0;

        game.t.press_spectate(goalie);
        game.tick();
        assert!(game.t.ctx().objects.skater(goalie).is_none());
        game.t.input(goalie).keys = 0;
        game.t.press_join(goalie, Team::Red);
        game.tick();
        assert!(game.t.ctx().objects.skater(goalie).is_some());
        assert!(!is_goalie(&game, goalie));
    }

    #[test]
    fn two_line_pass_warning_keeps_the_players_offside_at_the_first_line() {
        let mut server = crate::server::test_server();
        let passer = add_player(&mut server, "passer", 1);
        let past_center = add_player(&mut server, "past center", 2);
        let past_blue_line = add_player(&mut server, "past blue line", 3);
        let mut ctx = server.context();
        // Red attacks towards the blue zone, at low z.
        let length = ctx.rink.length;
        let center_z = ctx.rink.center_line.z;
        let blue_line_z = ctx.rink.blue_zone_blue_line.z;
        for (player, z) in [
            (passer, length - 15.0),
            (past_center, center_z - 3.0),
            (past_blue_line, blue_line_z - 3.0),
        ] {
            let pos = Vec3::new(15.0, 1.5, z);
            assert!(ctx.spawn_skater(player, Team::Red, pos, Rot3::IDENTITY, false));
        }
        let config = MatchConfiguration {
            twoline_pass: TwoLinePassConfiguration::Double,
            ..MatchConfiguration::default()
        };
        let mut playing = PlayingState {
            icing: Icing::No,
            offside: Offside::Neutral,
            twoline: TwoLine::No,
            pass: Some(Pass {
                team: Team::Red,
                side: RinkSide::Lower,
                from: Some(PassLocation::ReachedOwnBlue),
                player: passer,
            }),
            started_goalies: HashSet::new(),
            puck_touches: HashMap::new(),
            faceoff_step: 0,
        };

        // The pass crosses the center line, and then the offensive blue line.
        let mut match_events = Vec::new();
        PlayingState::enter_zone(
            &config,
            &mut ctx,
            &mut playing,
            Team::Red,
            false,
            &mut match_events,
        );
        PlayingState::enter_zone(
            &config,
            &mut ctx,
            &mut playing,
            Team::Red,
            true,
            &mut match_events,
        );
        assert_eq!(match_events, []);

        let TwoLine::Warning(warning) = &playing.twoline else {
            panic!("expected a two-line pass warning");
        };
        let mut offside_players = warning.twoline_offside_players.clone();
        offside_players.sort_by_key(|player| player.index.0);
        assert_eq!(offside_players, [past_center, past_blue_line]);
    }

    #[test]
    fn deciding_goal_shows_game_over_at_once_with_a_single_pause() {
        let mut server = crate::server::test_server();
        let mut ctx = server.context();
        let config = MatchConfiguration {
            first_to: 2,
            ..MatchConfiguration::default()
        };
        let settings = MatchSettings::new(config);
        let mut game = Match::start(&config, (1, 0));
        game.apply(
            &mut ctx,
            &settings,
            MatchTransition::StartPlaying {
                faceoff: Faceoff::Center,
                reset_clock: true,
            },
        );
        let (puck, _) = ctx.objects.pucks().next().unwrap();
        let MatchState::Playing(playing) = &mut game.state else {
            panic!("expected play to be on");
        };

        // The equalizer does not decide the game, so play goes on after the break.
        let stoppage = PlayingState::goal(
            &mut game.game_state,
            &config,
            &mut ctx,
            playing,
            Team::Blue,
            puck,
            &mut Vec::new(),
        );
        assert!(matches!(
            stoppage.kind,
            BreakKind::Stoppage(Stoppage {
                cause: StoppageCause::Goal,
                ..
            })
        ));
        assert_eq!(stoppage.timer, config.time_break * 100);
        assert_eq!(stoppage.scoreboard(), (RulesState::default(), 1000, false));

        // The winner shows game over and the goal message together, for the longer of the break
        // and the intermission, and then the game ends.
        let game_over = PlayingState::goal(
            &mut game.game_state,
            &config,
            &mut ctx,
            playing,
            Team::Red,
            puck,
            &mut Vec::new(),
        );
        assert_eq!(game.game_state.score, (2, 1));
        assert!(matches!(
            game_over.kind,
            BreakKind::GameOver { after_goal: true }
        ));
        let pause = config.time_break.max(config.time_intermission) * 100;
        assert_eq!(game_over.timer, pause);
        assert_eq!(game_over.scoreboard(), (RulesState::default(), pause, true));
        let mut last_tick = BreakState {
            timer: 1,
            ..game_over
        };
        assert!(matches!(
            last_tick.before_tick(&config, true),
            BreakOutcome::GameFinished
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
    fn goal_stoppage_follows_the_replay_ticks_that_were_queued() {
        let mut t = TestServer::new(EmptyGameMode);
        let player = t.join("player");
        let history = &mut t.server.state.replay;
        history.set_history_capacity(3);
        for _ in 0..5 {
            history.advance_step();
            history.record_tick(
                std::array::from_fn(|_| crate::protocol::ObjectPacket::None),
                std::array::from_fn(|_| None),
            );
        }
        t.take_messages(player);
        let config = MatchConfiguration::default();
        let goal_stoppage = |timer| {
            let mut game = Match::start(&config, (0, 0));
            game.state = MatchState::Break(BreakState::new(
                timer,
                BreakKind::Stoppage(Stoppage {
                    cause: StoppageCause::Goal,
                    next_faceoff: Faceoff::Center,
                }),
            ));
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

    /// Hands `events` to the running match as if the physics had reported them this tick, and
    /// returns the match events.
    fn after_tick_with(game: &mut MatchGame, events: &[PhysicsEvent]) -> Vec<MatchEvent> {
        let StandardMatchGameMode {
            settings, state, ..
        } = &mut game.t.mode.mode;
        let StandardMatchState::Game(running) = state else {
            panic!("no match is running");
        };
        running.after_tick(&mut game.t.server.context(), settings, events)
    }

    fn puck_id(game: &mut MatchGame) -> PuckId {
        game.t.ctx().objects.pucks().next().unwrap().0
    }

    /// A match with the given score, with play on after the opening faceoff.
    fn playing_match(ctx: &mut GameContext, settings: &MatchSettings, score: (u32, u32)) -> Match {
        let mut game = Match::start(&settings.config, score);
        game.apply(
            ctx,
            settings,
            MatchTransition::StartPlaying {
                faceoff: Faceoff::Center,
                reset_clock: true,
            },
        );
        game
    }

    #[test]
    fn period_end_is_returned_from_the_next_after_tick() {
        let mut server = crate::server::test_server();
        let mut ctx = server.context();
        let settings = MatchSettings::new(MatchConfiguration::default());
        let mut game = playing_match(&mut ctx, &settings, (0, 0));
        game.game_state.time = 1;

        assert_eq!(
            game.before_tick(&mut ctx, &settings),
            GameProgress::Continue
        );
        assert_eq!(
            game.after_tick(&mut ctx, &settings, &[]),
            [MatchEvent::PeriodEnded { period: 1 }]
        );
        assert_eq!(game.after_tick(&mut ctx, &settings, &[]), []);
    }

    #[test]
    fn last_period_ending_with_a_lead_ends_the_game() {
        let mut server = crate::server::test_server();
        let mut ctx = server.context();
        let settings = MatchSettings::new(MatchConfiguration {
            periods: 1,
            ..MatchConfiguration::default()
        });
        let mut game = playing_match(&mut ctx, &settings, (0, 2));
        game.game_state.time = 1;

        game.before_tick(&mut ctx, &settings);
        assert!(game.is_game_over());
        assert_eq!(
            game.after_tick(&mut ctx, &settings, &[]),
            [
                MatchEvent::PeriodEnded { period: 1 },
                MatchEvent::GameOver {
                    winner: Some(Team::Blue)
                },
            ]
        );
    }

    #[test]
    fn deciding_goal_is_followed_by_game_over() {
        let mut server = crate::server::test_server();
        let mut ctx = server.context();
        let settings = MatchSettings::new(MatchConfiguration {
            first_to: 2,
            ..MatchConfiguration::default()
        });
        let mut game = playing_match(&mut ctx, &settings, (1, 0));
        let (puck, _) = ctx.objects.pucks().next().unwrap();

        let events = game.after_tick(
            &mut ctx,
            &settings,
            &[PhysicsEvent::PuckEnteredNet {
                team: Team::Blue,
                puck,
            }],
        );
        assert!(
            matches!(
                events.as_slice(),
                [
                    MatchEvent::Goal {
                        team: Team::Red,
                        ..
                    },
                    MatchEvent::GameOver {
                        winner: Some(Team::Red)
                    },
                ]
            ),
            "{events:?}"
        );
    }

    #[test]
    fn goal_is_returned_with_the_scorer() {
        let mut game = MatchGame::start(MatchGame::quick_config(), 1, 1);
        let red = game.red[0];
        game.touch(red);
        let puck = puck_id(&mut game);

        assert_eq!(after_tick_with(&mut game, &[]), []);
        let events = after_tick_with(
            &mut game,
            &[PhysicsEvent::PuckEnteredNet {
                team: Team::Blue,
                puck,
            }],
        );
        assert!(
            matches!(
                events.as_slice(),
                [MatchEvent::Goal {
                    team: Team::Red,
                    scorer: Some(scorer),
                    assist: None,
                    speed_from_stick: Some(_),
                    ..
                }] if *scorer == red
            ),
            "{events:?}"
        );
    }

    #[test]
    fn icing_call_is_returned() {
        let config = MatchConfiguration {
            icing: IcingConfiguration::NoTouch,
            ..MatchGame::quick_config()
        };
        let mut game = MatchGame::start(config, 1, 1);
        game.touch(game.red[0]);
        let puck = puck_id(&mut game);

        let events = after_tick_with(
            &mut game,
            &[
                PhysicsEvent::PuckReachedDefensiveLine {
                    team: Team::Red,
                    puck,
                },
                PhysicsEvent::PuckPassedGoalLine {
                    team: Team::Blue,
                    puck,
                },
            ],
        );
        assert_eq!(events, [MatchEvent::Icing { team: Team::Red }]);
    }

    #[test]
    fn goal_scored_by_an_offside_team_is_returned_as_offside() {
        let config = MatchConfiguration {
            offside: OffsideConfiguration::Delayed,
            ..MatchGame::quick_config()
        };
        let mut game = MatchGame::start(config, 2, 1);
        game.clear_ice();
        game.place(game.red[1], RECEIVER_LANE, red_attack::IN_ZONE);
        game.place(game.red[0], PUCK_LANE, red_attack::PAST_CENTER);
        game.touch(game.red[0]);
        let puck = puck_id(&mut game);

        let zone_entry = [PhysicsEvent::PuckEnteredOffensiveZone {
            team: Team::Red,
            puck,
        }];
        assert_eq!(after_tick_with(&mut game, &zone_entry), []);
        let events = after_tick_with(
            &mut game,
            &[PhysicsEvent::PuckEnteredNet {
                team: Team::Blue,
                puck,
            }],
        );
        assert_eq!(events, [MatchEvent::Offside { team: Team::Red }]);
    }

    /// Red passes from the neutral zone into Blue's zone, where a red teammate already is.
    fn offside_pass(config: MatchConfiguration) -> MatchGame {
        offside_pass_from(config, red_attack::PAST_CENTER)
    }

    /// Red passes from `from_z` into Blue's zone, where a red teammate already is.
    fn offside_pass_from(config: MatchConfiguration, from_z: f32) -> MatchGame {
        let mut game = MatchGame::start(config, 2, 1);
        game.clear_ice();
        game.place(game.red[1], RECEIVER_LANE, red_attack::IN_ZONE);
        game.pass_from(game.red[0], PUCK_LANE, from_z, 0.3);
        game.wait_for_puck_past(red_attack::IN_ZONE + 3.0);
        game
    }

    /// Plays until the next faceoff, and checks that the puck is dropped at `faceoff`.
    fn assert_next_faceoff(game: &mut MatchGame, faceoff: Faceoff) {
        game.play_until_faceoff();
        let spot = faceoff_spot(&game.t.server.rink, faceoff, 0.0, 0.0).center;
        let puck = game.pucks()[0];
        assert!(
            Vec3::new(puck.x, 0.0, puck.z).distance(spot) < 0.1,
            "puck at {puck}, {faceoff:?} at {spot}"
        );
    }

    /// The passer goes to the puck and touches it again.
    fn passer_touches_the_puck(game: &mut MatchGame) {
        let puck = game.pucks()[0];
        game.place(game.red[0], puck.x, puck.z);
        game.touch(game.red[0]);
    }

    #[test]
    fn offside_is_faced_off_in_the_zone_the_pass_came_from() {
        // IIHF rule 83.6. Passes go along PUCK_LANE, on the higher side of the rink.
        let config = MatchConfiguration {
            offside: OffsideConfiguration::Immediate,
            ..MatchGame::quick_config()
        };
        for (from_z, faceoff) in [
            // As when carrying the puck in: the neutral zone spot nearest the attacking zone.
            (
                red_attack::PAST_CENTER,
                Faceoff::Offside(Team::Blue, RinkSide::Higher),
            ),
            (
                red_attack::OWN_HALF,
                Faceoff::Offside(Team::Red, RinkSide::Higher),
            ),
            (
                red_attack::OWN_ZONE,
                Faceoff::Defensive(Team::Red, RinkSide::Higher),
            ),
        ] {
            let mut game = offside_pass_from(config, from_z);
            assert!(game.heard("Offside"), "{:?}", game.heard);
            assert_next_faceoff(&mut game, faceoff);
        }
    }

    #[test]
    fn delayed_offside_on_a_dump_in_is_faced_off_where_it_came_from_when_the_passer_touches_it() {
        let config = MatchConfiguration {
            offside: OffsideConfiguration::Delayed,
            ..MatchGame::quick_config()
        };
        let mut game = offside_pass_from(config, red_attack::OWN_ZONE);
        assert!(game.heard("Offside warning"), "{:?}", game.heard);

        passer_touches_the_puck(&mut game);
        assert!(game.heard("Offside"), "{:?}", game.heard);
        assert_next_faceoff(&mut game, Faceoff::Defensive(Team::Red, RinkSide::Higher));
    }

    #[test]
    fn center_line_offside_is_faced_off_in_the_offending_teams_half() {
        let config = MatchConfiguration {
            offside: OffsideConfiguration::Delayed,
            offside_line: OffsideLineConfiguration::Center,
            ..MatchGame::quick_config()
        };
        let mut game = MatchGame::start(config, 2, 1);
        game.clear_ice();
        game.place(game.red[1], RECEIVER_LANE, red_attack::PAST_CENTER);
        game.pass_from(game.red[0], PUCK_LANE, red_attack::OWN_HALF, 0.3);
        game.wait_for_puck_past(red_attack::PAST_CENTER);
        assert!(game.heard("Offside warning"), "{:?}", game.heard);

        // Not at center, even though this looks like the passer carrying the puck over the line.
        passer_touches_the_puck(&mut game);
        assert!(game.heard("Offside"), "{:?}", game.heard);
        assert_next_faceoff(&mut game, Faceoff::Offside(Team::Red, RinkSide::Higher));
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
}

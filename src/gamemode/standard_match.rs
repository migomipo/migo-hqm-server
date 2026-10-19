use std::collections::{HashMap, HashSet, VecDeque};

use glam::Vec3;
use glamx::Rot3;
use reborrow::{Reborrow, ReborrowMut};

use crate::game::{PhysicsEvent, PlayerId, PlayerIndex, Puck, RulesState, ScoreboardValues, Team};
use crate::gamemode::util::{SpawnPoint, TeamSelection, get_spawnpoint};
use crate::gamemode::{ExitReason, GameMode, Server, ServerMut, ServerMutParts};

mod commands;
mod faceoff;

use faceoff::{assign_team_positions, faceoff_spot};

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
    pub time_warmup: u32,
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
            time_warmup: 300,
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
#[derive(Debug, Clone)]
enum PlayingTransition {
    Continue,
    Break(BreakState),
}
#[derive(Debug, Clone)]
enum BreakTransition {
    Continue,
    StartPlaying { faceoff: Faceoff, reset_clock: bool },
    NewGame,
}
#[derive(Debug)]
struct PlayingState {
    icing: Icing,
    offside: Offside,
    twoline: TwoLine,
    pass: Option<Pass>,
    started_goalies: HashSet<PlayerId>,
    puck_touches: HashMap<usize, VecDeque<PuckTouch>>,
    faceoff_step: u32,
}
impl BreakState {
    fn before_tick(&mut self, config: &MatchConfiguration, game_over: bool) -> BreakTransition {
        self.timer = self.timer.saturating_sub(1);
        if self.timer != 0 {
            return BreakTransition::Continue;
        }
        match &self.kind {
            BreakKind::Stoppage(_) if game_over => {
                self.timer = config.time_intermission * 100;
                self.kind = BreakKind::GameOver;
                BreakTransition::Continue
            }
            BreakKind::Stoppage(stoppage) => BreakTransition::StartPlaying {
                faceoff: stoppage.next_faceoff,
                reset_clock: false,
            },
            BreakKind::Intermission { .. } => BreakTransition::StartPlaying {
                faceoff: Faceoff::Center,
                reset_clock: true,
            },
            BreakKind::GameOver => BreakTransition::NewGame,
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
    fn handle_events(&mut self, mut server: ServerMut, events: &[PhysicsEvent]) {
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
        let elapsed = server
            .state()
            .replay()
            .game_step()
            .saturating_sub(*period_end_step);
        if elapsed <= 300 {
            *too_late_printed = true;
            server.add_server_chat_message(format!(
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
    pub(crate) config: MatchConfiguration,
    pub(crate) score: (u32, u32),
    pub(crate) period: u32,
    pub(crate) time: u32,
    replay: Option<(u32, u32, Option<PlayerId>)>,
    preferred_positions: HashMap<PlayerId, &'static str>,
}
impl MatchGameState {
    fn game_over(&self) -> bool {
        (self.period > self.config.periods && self.score.0 != self.score.1)
            || (self.config.mercy > 0 && self.score.0.abs_diff(self.score.1) >= self.config.mercy)
            || (self.config.first_to > 0
                && (self.score.0 >= self.config.first_to || self.score.1 >= self.config.first_to))
    }
}

impl Match {
    fn start(
        config: MatchConfiguration,
        preferred_positions: HashMap<PlayerId, &'static str>,
        score: (u32, u32),
    ) -> Self {
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
                config,
                score,
                period: 1,
                replay: None,
                preferred_positions,
            },
        }
    }
    fn setup_faceoff(
        mut server: ServerMut,
        config: &MatchConfiguration,
        preferred_positions: &HashMap<PlayerId, &'static str>,
        faceoff: Faceoff,
    ) -> PlayingState {
        server.state_mut().objects_mut().remove_all_pucks();
        let rink = server.rink();
        let spot = faceoff_spot(
            rink,
            faceoff,
            config.spawn_point_offset,
            config.spawn_player_altitude,
        );
        let puck_pos = spot.center + Vec3::Y * config.spawn_puck_altitude;
        server
            .state_mut()
            .objects_mut()
            .spawn_puck(Puck::new(puck_pos, Rot3::IDENTITY));
        let mut started_goalies = HashSet::new();
        let mut red_players = Vec::new();
        let mut blue_players = Vec::new();
        let players: Vec<_> = server
            .state()
            .players()
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
            server
                .state_mut()
                .spawn_skater(id, team, pos, rot, config.spawn_keep_stick_position);
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
            faceoff_step: server.state().replay().game_step(),
        }
    }
}

impl PlayingState {
    fn players_past(
        server: Server,
        team: Team,
        ignore: Option<PlayerId>,
        center: bool,
    ) -> Vec<PlayerId> {
        let line = if center {
            &server.rink().center_line
        } else {
            match team {
                Team::Red => &server.rink().blue_zone_blue_line,
                Team::Blue => &server.rink().red_zone_blue_line,
            }
        };
        server
            .state()
            .players()
            .iter()
            .filter_map(|p| {
                if Some(p.id) == ignore || p.team() != Some(team) {
                    None
                } else {
                    server
                        .state()
                        .skater_feet_position(p.id)
                        .filter(|pos| match team {
                            Team::Red => pos.z < line.z - line.width,
                            Team::Blue => pos.z > line.z + line.width / 2.0,
                        })
                        .map(|_| p.id)
                }
            })
            .collect()
    }
    fn team_in_zone(server: Server, team: Team, ignore: Option<PlayerId>) -> bool {
        !Self::players_past(server, team, ignore, false).is_empty()
    }
    fn side(server: Server, puck: usize) -> RinkSide {
        server
            .state()
            .objects()
            .get_puck(puck)
            .map_or(RinkSide::Lower, |p| {
                if p.body.pos.x <= server.rink().width / 2.0 {
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
        mut server: ServerMut,
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
        server.add_server_chat_message("Offside");
        Self::stoppage(
            config.time_break * 100,
            StoppageCause::Offside,
            next_faceoff,
        )
    }
    fn call_icing(
        config: &MatchConfiguration,
        mut server: ServerMut,
        team: Team,
        side: RinkSide,
    ) -> BreakState {
        let next_faceoff = Faceoff::Defensive(team, side);
        server.add_server_chat_message("Icing");
        Self::stoppage(config.time_break * 100, StoppageCause::Icing, next_faceoff)
    }
    fn call_twoline(
        config: &MatchConfiguration,
        mut server: ServerMut,
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
        server.add_server_chat_message("Two-line pass");
        Self::stoppage(
            config.time_break * 100,
            StoppageCause::Offside,
            next_faceoff,
        )
    }
    fn goal(
        game_state: &mut MatchGameState,
        mut server: ServerMut,
        playing: &mut PlayingState,
        team: Team,
        puck: usize,
    ) -> BreakState {
        if let Offside::Warning(warning) = playing.offside {
            if warning.offending_team == team {
                return Self::call_offside(
                    &game_state.config,
                    server,
                    team,
                    warning.side,
                    warning.pass_origin,
                    false,
                );
            }
        }
        let puck_speed_across_line = server
            .state()
            .objects()
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
        server
            .state_mut()
            .add_goal_message(team, goal_scorer, assist);
        let unit = if game_state.config.use_mph {
            "mph"
        } else {
            "km/h"
        };
        let speed_factor = if game_state.config.use_mph {
            223.693
        } else {
            360.0
        };
        let from_stick = puck_speed_from_stick
            .map(|speed| format!(", {:.1} {unit} from stick", speed * speed_factor))
            .unwrap_or_default();
        server.add_server_chat_message(format!(
            "Goal scored, {:.1} {unit} across line{from_stick}",
            puck_speed_across_line * speed_factor,
        ));
        if game_state.time < 1000 {
            server.add_server_chat_message(format!(
                "{}.{:02} seconds left",
                game_state.time / 100,
                game_state.time % 100,
            ));
        }
        let mut stoppage_time = game_state.config.time_break * 100;
        if game_state.config.goal_replay {
            let now = server.state().replay().game_step();
            game_state.replay = Some((
                playing.faceoff_step.max(now.saturating_sub(600)),
                now + 200,
                goal_scorer.or(last_touch),
            ));
            stoppage_time = stoppage_time.saturating_sub(800).max(400);
        }
        Self::stoppage(stoppage_time, StoppageCause::Goal, Faceoff::Center)
    }
    fn touch(
        game_state: &MatchGameState,
        mut server: ServerMut,
        playing: &mut PlayingState,
        player: PlayerId,
        puck: usize,
    ) -> Option<BreakState> {
        let Some(team) = server.state().players().get(player).and_then(|p| p.team()) else {
            return None;
        };
        let side = Self::side(server.rb(), puck);
        if let Some(puck_object) = server.state().objects().get_puck(puck) {
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
                    &game_state.config,
                    server,
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
                    &game_state.config,
                    server,
                    team,
                    warning.side,
                    warning.pass_origin,
                ));
            }
            playing.twoline = TwoLine::No;
            server.add_server_chat_message("Two-line pass waved off");
        }
        if let Icing::Warning(warning) = playing.icing {
            if team != warning.icing_team && !playing.started_goalies.contains(&player) {
                return Some(Self::call_icing(
                    &game_state.config,
                    server,
                    warning.icing_team,
                    warning.side,
                ));
            } else {
                playing.icing = Icing::No;
                server.add_server_chat_message("Icing waved off");
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
    fn wave_off_twoline(mut server: ServerMut, playing: &mut PlayingState, team: Team) {
        if let TwoLine::Warning(warning) = &playing.twoline {
            if warning.passing_team != team {
                playing.twoline = TwoLine::No;
                server.add_server_chat_message("Two-line pass waved off");
            }
        }
    }
    fn enter_zone(
        config: &MatchConfiguration,
        mut server: ServerMut,
        playing: &mut PlayingState,
        team: Team,
        blue_line: bool,
    ) -> Option<BreakState> {
        let enabled = (blue_line && config.offside_line == OffsideLineConfiguration::OffensiveBlue)
            || (!blue_line && config.offside_line == OffsideLineConfiguration::Center);
        if enabled && !matches!(playing.offside, Offside::InZone(t) if t == team) {
            if let Some(pass) = playing.pass {
                if pass.team == team && Self::team_in_zone(server.rb(), team, Some(pass.player)) {
                    match config.offside {
                        OffsideConfiguration::Delayed => {
                            playing.offside = Offside::Warning(OffsideWarning {
                                offending_team: team,
                                side: pass.side,
                                pass_origin: pass.from,
                                passer: pass.player,
                            });
                            server.add_server_chat_message("Offside warning");
                        }
                        OffsideConfiguration::Immediate => {
                            return Some(Self::call_offside(
                                config,
                                server.rb_mut(),
                                team,
                                pass.side,
                                pass.from,
                                false,
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
                    Self::players_past(server.rb(), team, Some(pass.player), !blue_line);
                if !twoline_offside_players.is_empty() {
                    playing.twoline = TwoLine::Warning(TwoLineWarning {
                        passing_team: team,
                        side: pass.side,
                        pass_origin: pass.from.unwrap(),
                        twoline_offside_players,
                    });
                    server.add_server_chat_message("Two-line pass warning");
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
        mut server: ServerMut,
        events: &[PhysicsEvent],
        playing: &mut PlayingState,
    ) -> PlayingTransition {
        for event in events {
            match *event {
                PhysicsEvent::PuckTouch { player, puck } => {
                    if let Some(stoppage) =
                        Self::touch(game_state, server.rb_mut(), playing, player, puck)
                    {
                        return PlayingTransition::Break(stoppage);
                    }
                }
                PhysicsEvent::PuckEnteredNet { team, puck } => {
                    return PlayingTransition::Break(Self::goal(
                        game_state,
                        server.rb_mut(),
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
                            match game_state.config.icing {
                                IcingConfiguration::Touch => {
                                    playing.icing = Icing::Warning(IcingWarning {
                                        icing_team: pass.team,
                                        side: pass.side,
                                    });
                                    server.add_server_chat_message("Icing warning");
                                }
                                IcingConfiguration::NoTouch => {
                                    return PlayingTransition::Break(Self::call_icing(
                                        &game_state.config,
                                        server.rb_mut(),
                                        pass.team,
                                        pass.side,
                                    ));
                                }
                                IcingConfiguration::Off => {}
                            }
                        }
                    }
                }
                PhysicsEvent::PuckReachedDefensiveLine { team, .. } => {
                    Self::wave_off_twoline(server.rb_mut(), playing, team);
                    Self::update_pass(playing, team, PassLocation::ReachedOwnBlue)
                }
                PhysicsEvent::PuckPassedDefensiveLine { team, .. } => {
                    Self::update_pass(playing, team, PassLocation::PassedOwnBlue);
                    if game_state.config.offside_line == OffsideLineConfiguration::OffensiveBlue {
                        if let Offside::Warning(warning) = playing.offside {
                            if team.get_other_team() == warning.offending_team {
                                server.add_server_chat_message("Offside waved off");
                            }
                        }
                        playing.offside = Offside::Neutral;
                    }
                }
                PhysicsEvent::PuckReachedCenterLine { team, .. } => {
                    Self::wave_off_twoline(server.rb_mut(), playing, team);
                    Self::update_pass(playing, team, PassLocation::ReachedCenter)
                }
                PhysicsEvent::PuckPassedCenterLine { team, .. } => {
                    Self::update_pass(playing, team, PassLocation::PassedCenter);
                    if let Some(stoppage) =
                        Self::enter_zone(&game_state.config, server.rb_mut(), playing, team, false)
                    {
                        return PlayingTransition::Break(stoppage);
                    }
                    if let Offside::Warning(warning) = playing.offside {
                        if warning.offending_team != team {
                            server.add_server_chat_message("Offside waved off");
                        }
                    }
                }
                PhysicsEvent::PuckReachedOffensiveZone { team, .. } => {
                    Self::update_pass(playing, team, PassLocation::ReachedOffensive)
                }
                PhysicsEvent::PuckEnteredOffensiveZone { team, .. } => {
                    Self::update_pass(playing, team, PassLocation::PassedOffensive);
                    if let Some(stoppage) =
                        Self::enter_zone(&game_state.config, server.rb_mut(), playing, team, true)
                    {
                        return PlayingTransition::Break(stoppage);
                    }
                }
                _ => {}
            }
        }
        if let Offside::Warning(warning) = playing.offside {
            if !Self::team_in_zone(server.rb(), warning.offending_team, None) {
                playing.offside = Offside::InZone(warning.offending_team);
                server.add_server_chat_message("Offside waved off");
            }
        }
        PlayingTransition::Continue
    }
}

impl Match {
    fn before_tick(&mut self, mut server: ServerMut) {
        match &mut self.state {
            MatchState::Playing(_) => {
                self.game_state.time = self.game_state.time.saturating_sub(1);
                if self.game_state.time == 0 {
                    self.game_state.period += 1;
                    self.state = MatchState::Break(BreakState {
                        timer: self.game_state.config.time_intermission * 100,
                        kind: if self.game_state.game_over() {
                            BreakKind::GameOver
                        } else {
                            BreakKind::Intermission {
                                period_end_step: Some(server.state().replay().game_step()),
                                too_late_printed: false,
                            }
                        },
                    });
                }
            }
            MatchState::Break(state) => {
                let transition =
                    state.before_tick(&self.game_state.config, self.game_state.game_over());
                match transition {
                    BreakTransition::Continue => {}
                    BreakTransition::StartPlaying {
                        faceoff,
                        reset_clock,
                    } => {
                        if reset_clock {
                            self.game_state.time = self.game_state.config.time_period * 100;
                        }
                        self.state = MatchState::Playing(Self::setup_faceoff(
                            server.rb_mut(),
                            &self.game_state.config,
                            &self.game_state.preferred_positions,
                            faceoff,
                        ));
                    }
                    BreakTransition::NewGame => server.new_game(),
                }
            }
        }
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
    fn after_tick(&mut self, mut server: ServerMut, events: &[PhysicsEvent]) -> ScoreboardValues {
        let transition = {
            match &mut self.state {
                MatchState::Playing(playing) => {
                    PlayingState::events(&mut self.game_state, server.rb_mut(), events, playing)
                }
                MatchState::Break(state) => {
                    state.handle_events(server.rb_mut(), events);
                    PlayingTransition::Continue
                }
            }
        };
        if let PlayingTransition::Break(state) = transition {
            self.state = MatchState::Break(state);
        }
        if let Some((start_step, end_step, force_view)) = self.game_state.replay {
            if end_step <= server.state().replay().game_step() {
                server
                    .state_mut()
                    .replay_mut()
                    .add_replay_to_queue(start_step, end_step, force_view);
                server.add_server_chat_message("Goal replay");
                self.game_state.replay = None;
            }
        }
        self.scoreboard()
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
    fn new(time: u32) -> Self {
        Self {
            time,
            score: (0, 0),
        }
    }
    fn before_tick(&mut self, red_players: usize, blue_players: usize) -> Option<(u32, u32)> {
        if red_players == 0 || blue_players == 0 {
            self.time = 2000;
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
    pub config: MatchConfiguration,
    pub state: StandardMatchState,
    pub spawn_point: SpawnPoint,
    pub teams: TeamSelection,
    preferred_positions: HashMap<PlayerId, &'static str>,
}
impl StandardMatchGameMode {
    pub fn new(config: MatchConfiguration, team_max: usize, spawn_point: SpawnPoint) -> Self {
        Self {
            config,
            state: StandardMatchState::WaitingForGame(WarmupState::new(2000)),
            spawn_point,
            teams: TeamSelection::new(team_max),
            preferred_positions: HashMap::new(),
        }
    }
    fn update_players(&mut self, mut server: ServerMut) -> (usize, usize) {
        let point = self.spawn_point;
        let ServerMutParts { state, rink, .. } = server.as_mut_parts();
        self.teams
            .update(state, |team, _| get_spawnpoint(rink, team, point))
    }
    fn start_game(&mut self, preliminary_score: (u32, u32)) {
        self.state = StandardMatchState::Game(Match::start(
            self.config,
            self.preferred_positions.clone(),
            preliminary_score,
        ));
    }
    pub(crate) fn set_config(&mut self, update: impl Fn(&mut MatchConfiguration)) {
        update(&mut self.config);
        if let StandardMatchState::Game(game) = &mut self.state {
            update(&mut game.game_state.config);
        }
    }
}
impl GameMode for StandardMatchGameMode {
    fn init(&mut self, mut server: ServerMut) {
        server.state_mut().replay_mut().set_history_length(1000);
    }
    fn before_tick(&mut self, mut server: ServerMut) {
        let (red, blue) = self.update_players(server.rb_mut());
        let mut preliminary_score = None;
        match &mut self.state {
            StandardMatchState::WaitingForGame(warmup) => {
                preliminary_score = warmup.before_tick(red, blue);
            }
            StandardMatchState::Game(game) => game.before_tick(server.rb_mut()),
        }
        if let Some(score) = preliminary_score {
            self.start_game(score);
        }
    }
    fn after_tick(&mut self, server: ServerMut, events: &[PhysicsEvent]) -> ScoreboardValues {
        match &mut self.state {
            StandardMatchState::WaitingForGame(warmup) => warmup.scoreboard(),
            StandardMatchState::Game(game) => game.after_tick(server, events),
        }
    }
    fn handle_command(
        &mut self,
        mut server: ServerMut,
        command: &str,
        arg: &str,
        player: PlayerId,
    ) {
        match command {
            "set" => self.handle_set(server, arg, player),
            "sp" | "setposition" => {
                let requested = arg.to_ascii_uppercase();
                if let Some(position) = ALLOWED_POSITIONS
                    .iter()
                    .find(|position| **position == requested)
                {
                    self.preferred_positions.insert(player, position);
                    if let StandardMatchState::Game(game) = &mut self.state {
                        game.game_state.preferred_positions.insert(player, position);
                    }
                    server.add_directed_server_chat_message(
                        format!("Preferred faceoff position set to {position}"),
                        player,
                    );
                } else {
                    server.add_directed_server_chat_message("Invalid faceoff position", player);
                }
            }
            "fs" => {
                if let Ok(index) = arg.parse::<PlayerIndex>() {
                    if server.require_admin(player).is_some() {
                        let target = server.state().players().get_by_index(index).map(|p| p.id);
                        if let Some(target) = target {
                            self.teams.force_spectate(server.state_mut(), target);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    fn game_started(&mut self, mut server: ServerMut) {
        self.state = StandardMatchState::WaitingForGame(WarmupState::new(
            self.config.time_warmup.saturating_mul(100),
        ));
        let (width, length) = (server.rink().width, server.rink().length);
        let first_x = width / 2.0 - 0.4 * self.config.warmup_pucks.saturating_sub(1) as f32;
        for index in 0..self.config.warmup_pucks {
            server.state_mut().objects_mut().spawn_puck(Puck::new(
                Vec3::new(
                    first_x + 0.8 * index as f32,
                    self.config.spawn_puck_altitude,
                    length / 2.0,
                ),
                Rot3::IDENTITY,
            ));
        }
    }
    fn before_player_exit(&mut self, _server: ServerMut, player: PlayerId, _reason: ExitReason) {
        self.teams.remove_player(player);
        self.preferred_positions.remove(&player);
        if let StandardMatchState::Game(game) = &mut self.state {
            game.game_state.preferred_positions.remove(&player);
        }
    }
    fn server_list_team_size(&self) -> u32 {
        self.teams.team_max as u32
    }
    fn include_tick_in_recording(&self, _server: Server) -> bool {
        matches!(self.state, StandardMatchState::Game(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            BreakTransition::StartPlaying {
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
            BreakTransition::StartPlaying {
                faceoff: Faceoff::Center,
                reset_clock: true,
            }
        ));
    }

    #[test]
    fn match_start_enters_an_opening_break() {
        let config = MatchConfiguration::default();
        let game = Match::start(config, HashMap::new(), (0, 0));

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
}

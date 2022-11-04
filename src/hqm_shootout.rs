use migo_hqm_server::hqm_game::{HQMGame, HQMObjectIndex, HQMPhysicsConfiguration, HQMTeam};
use migo_hqm_server::hqm_server::{
    HQMServer, HQMServerBehaviour, HQMServerPlayerData, HQMServerPlayerIndex, HQMSpawnPoint,
};
use migo_hqm_server::hqm_simulate::HQMSimulationEvent;
use nalgebra::{Point3, Rotation3, Vector3};
use std::collections::HashMap;
use std::f32::consts::PI;
use std::rc::Rc;

use crate::hqm_behaviour_extra::HQMDualControlSetting;
use tracing::info;

enum HQMShootoutAttemptState {
    Attack { progress: f32 }, // Puck has been touched by attacker, but not touched by goalie, hit post or moved backwards
    NoMoreAttack { final_progress: f32 }, // Puck has moved backwards, hit the post or the goalie, but may still enter the net
    Over { timer: u32, goal_scored: bool }, // Attempt is over
}

enum HQMShootoutStatus {
    WaitingForGame,
    Game {
        state: HQMShootoutAttemptState,
        round: u32,
        team: HQMTeam,
    },
}

pub struct HQMShootoutBehaviour {
    attempts: u32,
    status: HQMShootoutStatus,
    physics_config: HQMPhysicsConfiguration,
    paused: bool,
    team_switch_timer: HashMap<HQMServerPlayerIndex, u32>,
    team_max: usize,
    pub dual_control: HQMDualControlSetting,
}

impl HQMShootoutBehaviour {
    pub fn new(
        attempts: u32,
        physics_config: HQMPhysicsConfiguration,
        dual_control: HQMDualControlSetting,
    ) -> Self {
        HQMShootoutBehaviour {
            attempts,
            status: HQMShootoutStatus::WaitingForGame,
            physics_config,
            paused: false,
            team_switch_timer: Default::default(),
            team_max: 1,
            dual_control,
        }
    }

    fn init(&mut self, server: &mut HQMServer) {
        self.start_next_attempt(server);
    }

    fn start_attempt(&mut self, server: &mut HQMServer, round: u32, team: HQMTeam) {
        self.status = HQMShootoutStatus::Game {
            state: HQMShootoutAttemptState::Attack { progress: 0.0 },
            round,
            team,
        };

        let defending_team = team.get_other_team();

        let remaining_attempts = self.attempts.saturating_sub(round);
        if remaining_attempts >= 2 {
            let msg = format!("{} attempts left for {}", remaining_attempts, team);
            server.messages.add_server_chat_message(msg);
        } else if remaining_attempts == 1 {
            let msg = format!("Last attempt for {}", team);
            server.messages.add_server_chat_message(msg);
        } else {
            let msg = format!("Tie-breaker round for {}", team);
            server.messages.add_server_chat_message(msg);
        }

        let mut red_players = vec![];
        let mut blue_players = vec![];

        for (player_index, player) in server.players.iter() {
            if let Some(player) = player {
                if let Some((_, team)) = player.object {
                    if team == HQMTeam::Red {
                        red_players.push(player_index);
                    } else if team == HQMTeam::Blue {
                        blue_players.push(player_index);
                    }
                }
            }
        }
        server.game.time = 2000;
        server.game.period = 1;
        server.game.world.clear_pucks();

        let length = server.game.world.rink.length;
        let width = server.game.world.rink.width;

        let puck_pos = Point3::new(width / 2.0, 1.0, length / 2.0);
        server
            .game
            .world
            .create_puck_object(puck_pos, Rotation3::identity());

        let red_rot = Rotation3::identity();
        let blue_rot = Rotation3::from_euler_angles(0.0, PI, 0.0);

        let red_goalie_pos = Point3::new(width / 2.0, 1.5, length - 5.0);
        let blue_goalie_pos = Point3::new(width / 2.0, 1.5, 5.0);
        let (attacking_players, defending_players, attacking_rot, defending_rot, goalie_pos) =
            match team {
                HQMTeam::Red => (
                    red_players,
                    blue_players,
                    red_rot,
                    blue_rot,
                    blue_goalie_pos,
                ),
                HQMTeam::Blue => (blue_players, red_players, blue_rot, red_rot, red_goalie_pos),
            };
        let center_pos = Point3::new(width / 2.0, 1.5, length / 2.0);
        for (index, player_index) in attacking_players.into_iter().enumerate() {
            let mut pos = center_pos + &attacking_rot * Vector3::new(0.0, 0.0, 3.0);
            if index > 0 {
                let dist = ((index / 2) + 1) as f32;

                let side = if index % 2 == 0 {
                    Vector3::new(-1.5 * dist, 0.0, 0.0)
                } else {
                    Vector3::new(-1.5 * dist, 0.0, 0.0)
                };
                pos += &attacking_rot * side;
            }
            server.spawn_skater(self, player_index, team, pos, attacking_rot.clone());
        }
        for (index, player_index) in defending_players.into_iter().enumerate() {
            let mut pos = goalie_pos.clone();
            if index > 0 {
                let dist = ((index / 2) + 1) as f32;

                let side = if index % 2 == 0 {
                    Vector3::new(-1.5 * dist, 0.0, 0.0)
                } else {
                    Vector3::new(-1.5 * dist, 0.0, 0.0)
                };
                pos += &defending_rot * side;
            }
            server.spawn_skater(
                self,
                player_index,
                defending_team,
                pos,
                defending_rot.clone(),
            );
        }
    }

    fn start_next_attempt(&mut self, server: &mut HQMServer) {
        let (next_team, next_round) = match &self.status {
            HQMShootoutStatus::WaitingForGame => (HQMTeam::Red, 0),
            HQMShootoutStatus::Game { team, round, .. } => (
                team.get_other_team(),
                if *team == HQMTeam::Blue {
                    *round + 1
                } else {
                    *round
                },
            ),
        };

        self.start_attempt(server, next_round, next_team);
    }

    fn update_players(&mut self, server: &mut HQMServer) {
        let mut spectating_players = vec![];
        let mut joining_red = vec![];
        let mut joining_blue = vec![];
        for (player_index, player) in server.players.iter() {
            if let Some(player) = player {
                self.team_switch_timer
                    .get_mut(&player_index)
                    .map(|x| *x = x.saturating_sub(1));
                if player.input.join_red() || player.input.join_blue() {
                    let has_skater = player.object.is_some()
                        || server.get_dual_control_player(player_index).is_some();
                    if !has_skater
                        && self
                            .team_switch_timer
                            .get(&player_index)
                            .map_or(true, |x| *x == 0)
                    {
                        let dual_control = self.dual_control == HQMDualControlSetting::Yes
                            || (self.dual_control == HQMDualControlSetting::Combined
                                && player.input.shift());
                        if player.input.join_red() {
                            joining_red.push((
                                player_index,
                                player.player_name.clone(),
                                dual_control,
                            ));
                        } else if player.input.join_blue() {
                            joining_blue.push((
                                player_index,
                                player.player_name.clone(),
                                dual_control,
                            ));
                        }
                    }
                } else if player.input.spectate() {
                    let has_skater = player.object.is_some()
                        || server.get_dual_control_player(player_index).is_some();
                    if has_skater {
                        self.team_switch_timer.insert(player_index, 500);
                        spectating_players.push((player_index, player.player_name.clone()))
                    }
                }
            }
        }
        for (player_index, player_name) in spectating_players {
            info!("{} ({}) is spectating", player_name, player_index);
            server.remove_player_from_dual_control(self, player_index);
            server.move_to_spectator(self, player_index);
        }
        if !joining_red.is_empty() || !joining_blue.is_empty() {
            let (red_player_count, blue_player_count) = {
                let mut red_player_count = 0usize;
                let mut blue_player_count = 0usize;
                for (_, player) in server.players.iter() {
                    if let Some(player) = player {
                        if let Some((_, team)) = player.object {
                            if team == HQMTeam::Red {
                                red_player_count += 1;
                            } else if team == HQMTeam::Blue {
                                blue_player_count += 1;
                            }
                        }
                    }
                }
                (red_player_count, blue_player_count)
            };
            let mut new_red_player_count = red_player_count;
            let mut new_blue_player_count = blue_player_count;

            fn add_player(
                behaviour: &mut HQMShootoutBehaviour,
                player_index: HQMServerPlayerIndex,
                player_name: Rc<String>,
                server: &mut HQMServer,
                team: HQMTeam,
                player_count: &mut usize,
                team_max: usize,
            ) {
                if *player_count >= team_max {
                    return;
                }

                if server
                    .spawn_skater_at_spawnpoint(behaviour, player_index, team, HQMSpawnPoint::Bench)
                    .is_some()
                {
                    info!(
                        "{} ({}) has joined team {:?}",
                        player_name, player_index, team
                    );
                    *player_count += 1
                }
            }
            fn add_player_dual_control(
                behaviour: &mut HQMShootoutBehaviour,
                player_index: HQMServerPlayerIndex,
                player_name: Rc<String>,
                server: &mut HQMServer,
                team: HQMTeam,
                player_count: &mut usize,
                team_max: usize,
            ) {
                let current_empty = find_empty_dual_control(server, team);

                match current_empty {
                    Some((index, movement @ Some(_), None)) => {
                        server.update_dual_control(behaviour, index, movement, Some(player_index));
                    }
                    Some((index, None, stick @ Some(_))) => {
                        server.update_dual_control(behaviour, index, Some(player_index), stick);
                    }
                    _ => {
                        if *player_count >= team_max {
                            return;
                        }

                        if server
                            .spawn_dual_control_skater_at_spawnpoint(
                                behaviour,
                                team,
                                HQMSpawnPoint::Bench,
                                Some(player_index),
                                None,
                            )
                            .is_some()
                        {
                            info!(
                                "{} ({}) has joined team {:?}",
                                player_name, player_index, team
                            );
                            *player_count += 1;
                        }
                    }
                }
            }

            for (player_index, player_name, dual_control) in joining_red {
                if dual_control {
                    add_player_dual_control(
                        self,
                        player_index,
                        player_name,
                        server,
                        HQMTeam::Red,
                        &mut new_red_player_count,
                        self.team_max,
                    );
                } else {
                    add_player(
                        self,
                        player_index,
                        player_name,
                        server,
                        HQMTeam::Red,
                        &mut new_red_player_count,
                        self.team_max,
                    );
                }
            }
            for (player_index, player_name, dual_control) in joining_blue {
                if dual_control {
                    add_player_dual_control(
                        self,
                        player_index,
                        player_name,
                        server,
                        HQMTeam::Blue,
                        &mut new_blue_player_count,
                        self.team_max,
                    );
                } else {
                    add_player(
                        self,
                        player_index,
                        player_name,
                        server,
                        HQMTeam::Blue,
                        &mut new_blue_player_count,
                        self.team_max,
                    );
                }
            }
        }
    }

    fn update_gameover(&mut self, server: &mut HQMServer) {
        if let HQMShootoutStatus::Game { state, team, round } = &mut self.status {
            let is_attempt_over = if matches!(state, HQMShootoutAttemptState::Over { .. }) {
                1
            } else {
                0
            };
            let red_attempts_taken = *round + is_attempt_over;
            let blue_attempts_taken = *round
                + match team {
                    HQMTeam::Red => 0,
                    HQMTeam::Blue => is_attempt_over,
                };
            let attempts = self.attempts.max(red_attempts_taken);
            let remaining_red_attempts = attempts - red_attempts_taken;
            let remaining_blue_attempts = attempts - blue_attempts_taken;

            server.game.game_over = if let Some(difference) =
                server.game.red_score.checked_sub(server.game.blue_score)
            {
                remaining_blue_attempts < difference
            } else if let Some(difference) =
                server.game.blue_score.checked_sub(server.game.red_score)
            {
                remaining_red_attempts < difference
            } else {
                false
            };
        }
    }

    fn end_attempt(&mut self, server: &mut HQMServer, goal_scored: bool) {
        if let HQMShootoutStatus::Game { state, team, .. } = &mut self.status {
            if goal_scored {
                match team {
                    HQMTeam::Red => {
                        server.game.red_score += 1;
                    }
                    HQMTeam::Blue => {
                        server.game.blue_score += 1;
                    }
                }
                server.messages.add_goal_message(*team, None, None);
            } else {
                server.messages.add_server_chat_message_str("Miss");
            }
            *state = HQMShootoutAttemptState::Over {
                timer: 500,
                goal_scored,
            };
            self.update_gameover(server);
        }
    }

    fn reset_game(&mut self, server: &mut HQMServer, player_index: HQMServerPlayerIndex) {
        if let Some(player) = server.players.get(player_index) {
            if player.is_admin {
                info!("{} ({}) reset game", player.player_name, player_index);
                let msg = format!("Game reset by {}", player.player_name);

                server.new_game(self.create_game());

                server.messages.add_server_chat_message(msg);
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    fn force_player_off_ice(
        &mut self,
        server: &mut HQMServer,
        admin_player_index: HQMServerPlayerIndex,
        force_player_index: HQMServerPlayerIndex,
    ) {
        if let Some(player) = server.players.get(admin_player_index) {
            if player.is_admin {
                let admin_player_name = player.player_name.clone();

                if let Some(force_player) = server.players.get(force_player_index) {
                    let force_player_name = force_player.player_name.clone();
                    if server.move_to_spectator(self, force_player_index) {
                        let msg = format!(
                            "{} forced off ice by {}",
                            force_player_name, admin_player_name
                        );
                        info!(
                            "{} ({}) forced {} ({}) off ice",
                            admin_player_name,
                            admin_player_index,
                            force_player_name,
                            force_player_index
                        );
                        server.messages.add_server_chat_message(msg);
                        self.team_switch_timer.insert(force_player_index, 500);
                    }
                }
            } else {
                server.admin_deny_message(admin_player_index);
                return;
            }
        }
    }

    fn set_score(
        &mut self,
        server: &mut HQMServer,
        input_team: HQMTeam,
        input_score: u32,
        player_index: HQMServerPlayerIndex,
    ) {
        if let Some(player) = server.players.get(player_index) {
            if player.is_admin {
                match input_team {
                    HQMTeam::Red => {
                        server.game.red_score = input_score;

                        info!(
                            "{} ({}) changed red score to {}",
                            player.player_name, player_index, input_score
                        );
                        let msg = format!("Red score changed by {}", player.player_name);
                        server.messages.add_server_chat_message(msg);
                    }
                    HQMTeam::Blue => {
                        server.game.blue_score = input_score;

                        info!(
                            "{} ({}) changed blue score to {}",
                            player.player_name, player_index, input_score
                        );
                        let msg = format!("Blue score changed by {}", player.player_name);
                        server.messages.add_server_chat_message(msg);
                    }
                }
                self.update_gameover(server);
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    fn set_round(
        &mut self,
        server: &mut HQMServer,
        input_team: HQMTeam,
        input_round: u32,
        player_index: HQMServerPlayerIndex,
    ) {
        if input_round == 0 {
            return;
        }
        if let Some(player) = server.players.get(player_index) {
            if player.is_admin {
                if let HQMShootoutStatus::Game {
                    state: _,
                    round,
                    team,
                } = &mut self.status
                {
                    *round = input_round - 1;
                    *team = input_team;

                    info!(
                        "{} ({}) changed round to {} for {}",
                        player.player_name, player_index, input_round, input_team
                    );
                    let msg = format!(
                        "Round changed to {} for {} by {}",
                        input_round, input_team, player.player_name
                    );
                    server.messages.add_server_chat_message(msg);
                }
                self.update_gameover(server);
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    fn redo_round(
        &mut self,
        server: &mut HQMServer,
        input_team: HQMTeam,
        input_round: u32,
        player_index: HQMServerPlayerIndex,
    ) {
        if input_round == 0 {
            return;
        }
        if let Some(player) = server.players.get(player_index) {
            if player.is_admin {
                if let HQMShootoutStatus::Game {
                    state: _,
                    round,
                    team,
                } = &mut self.status
                {
                    *round = input_round - 1;
                    *team = input_team;
                }
                info!(
                    "{} ({}) changed round to {} for {}",
                    player.player_name, player_index, input_round, input_team
                );
                let msg = format!(
                    "Round changed to {} for {} by {}",
                    input_round, input_team, player.player_name
                );
                server.messages.add_server_chat_message(msg);
                self.update_gameover(server);
                self.paused = false;
                if !server.game.game_over {
                    self.start_attempt(server, input_round - 1, input_team);
                }
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    fn pause(&mut self, server: &mut HQMServer, player_index: HQMServerPlayerIndex) {
        if let Some(player) = server.players.get(player_index) {
            if player.is_admin {
                self.paused = true;

                info!("{} ({}) paused game", player.player_name, player_index);
                let msg = format!("Game paused by {}", player.player_name);
                server.messages.add_server_chat_message(msg);
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }

    fn unpause(&mut self, server: &mut HQMServer, player_index: HQMServerPlayerIndex) {
        if let Some(player) = server.players.get(player_index) {
            if player.is_admin {
                self.paused = false;
                if let HQMShootoutStatus::Game {
                    state: HQMShootoutAttemptState::Over { timer, .. },
                    ..
                } = &mut self.status
                {
                    *timer = (*timer).max(2000);
                }
                info!("{} ({}) resumed game", player.player_name, player_index);
                let msg = format!("Game resumed by {}", player.player_name);

                server.messages.add_server_chat_message(msg);
            } else {
                server.admin_deny_message(player_index);
            }
        }
    }
}

impl HQMServerBehaviour for HQMShootoutBehaviour {
    fn before_tick(&mut self, server: &mut HQMServer) {
        self.update_players(server);
    }

    fn after_tick(&mut self, server: &mut HQMServer, events: &[HQMSimulationEvent]) {
        for event in events {
            match event {
                HQMSimulationEvent::PuckEnteredNet {
                    team: scoring_team, ..
                } => {
                    if let HQMShootoutStatus::Game {
                        state,
                        team: attacking_team,
                        ..
                    } = &mut self.status
                    {
                        if let HQMShootoutAttemptState::Over { .. } = *state {
                            // Ignore
                        } else {
                            let is_goal = *scoring_team == *attacking_team;
                            self.end_attempt(server, is_goal);
                        }
                    }
                }
                HQMSimulationEvent::PuckPassedGoalLine { .. } => {
                    if let HQMShootoutStatus::Game { state, .. } = &mut self.status {
                        if let HQMShootoutAttemptState::Over { .. } = *state {
                            // Ignore
                        } else {
                            self.end_attempt(server, false);
                        }
                    }
                }
                HQMSimulationEvent::PuckTouch { player, .. } => {
                    let player = *player;
                    if let Some((_, touching_team, _)) =
                        server.players.get_from_object_index(player)
                    {
                        if let HQMShootoutStatus::Game {
                            state,
                            team: attacking_team,
                            ..
                        } = &mut self.status
                        {
                            if touching_team == *attacking_team {
                                if let HQMShootoutAttemptState::NoMoreAttack { .. } = *state {
                                    self.end_attempt(server, false);
                                }
                            } else {
                                if let HQMShootoutAttemptState::Attack { progress } = *state {
                                    *state = HQMShootoutAttemptState::NoMoreAttack {
                                        final_progress: progress,
                                    }
                                }
                            }
                        }
                    }
                }
                HQMSimulationEvent::PuckTouchedNet { team, .. } => {
                    if let HQMShootoutStatus::Game {
                        state,
                        team: attacking_team,
                        ..
                    } = &mut self.status
                    {
                        if *team == *attacking_team {
                            if let HQMShootoutAttemptState::Attack { progress } = *state {
                                *state = HQMShootoutAttemptState::NoMoreAttack {
                                    final_progress: progress,
                                };
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        match &mut self.status {
            HQMShootoutStatus::WaitingForGame => {
                let (red_player_count, blue_player_count) = {
                    let mut red_player_count = 0usize;
                    let mut blue_player_count = 0usize;
                    for (_, player) in server.players.iter() {
                        if let Some(player) = player {
                            if let Some((_, team)) = player.object {
                                if team == HQMTeam::Red {
                                    red_player_count += 1;
                                } else if team == HQMTeam::Blue {
                                    blue_player_count += 1;
                                }
                            }
                        }
                    }
                    (red_player_count, blue_player_count)
                };
                if red_player_count > 0 && blue_player_count > 0 && !self.paused {
                    server.game.time = server.game.time.saturating_sub(1);
                    if server.game.time == 0 {
                        self.init(server);
                    }
                } else {
                    server.game.time = 1000;
                }
            }
            HQMShootoutStatus::Game { state, team, .. } => {
                if !self.paused {
                    if let HQMShootoutAttemptState::Over { timer, goal_scored } = state {
                        *timer = timer.saturating_sub(1);
                        server.game.goal_message_timer = if *goal_scored { *timer } else { 0 };
                        if *timer == 0 {
                            if server.game.game_over {
                                let new_game = self.create_game();
                                server.new_game(new_game);
                            } else {
                                self.start_next_attempt(server);
                            }
                        }
                    } else {
                        server.game.time = server.game.time.saturating_sub(1);
                        if server.game.time == 0 {
                            server.game.time = 1; // A hack to avoid "Intermission" or "Game starting"
                            self.end_attempt(server, false);
                        } else {
                            if let Some(puck) =
                                server.game.world.objects.get_puck(HQMObjectIndex(0))
                            {
                                let puck_pos = &puck.body.pos;
                                let center_pos =
                                    &server.game.world.rink.center_faceoff_spot.center_position;
                                let pos_diff = puck_pos - center_pos;
                                let normal = match *team {
                                    HQMTeam::Red => -Vector3::z(),
                                    HQMTeam::Blue => Vector3::z(),
                                };
                                let progress = pos_diff.dot(&normal);
                                if let HQMShootoutAttemptState::Attack {
                                    progress: current_progress,
                                } = state
                                {
                                    if progress > *current_progress {
                                        *current_progress = progress;
                                    } else if progress - *current_progress < -0.5 {
                                        // Too far back
                                        self.end_attempt(server, false);
                                    }
                                } else if let HQMShootoutAttemptState::NoMoreAttack {
                                    final_progress,
                                } = *state
                                {
                                    if progress - final_progress < -5.0 {
                                        self.end_attempt(server, false);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    fn handle_command(
        &mut self,
        server: &mut HQMServer,
        cmd: &str,
        arg: &str,
        player_index: HQMServerPlayerIndex,
    ) {
        match cmd {
            "reset" | "resetgame" => {
                self.reset_game(server, player_index);
            }
            "fs" => {
                if let Ok(force_player_index) = arg.parse::<HQMServerPlayerIndex>() {
                    self.force_player_off_ice(server, player_index, force_player_index);
                }
            }
            "set" => {
                let args = arg.split(" ").collect::<Vec<&str>>();
                if args.len() >= 2 {
                    match args[0] {
                        "redscore" => {
                            if let Ok(input_score) = args[1].parse::<u32>() {
                                self.set_score(server, HQMTeam::Red, input_score, player_index);
                            }
                        }
                        "bluescore" => {
                            if let Ok(input_score) = args[1].parse::<u32>() {
                                self.set_score(server, HQMTeam::Blue, input_score, player_index);
                            }
                        }
                        "round" => {
                            if args.len() >= 3 {
                                let team = match args[1] {
                                    "r" | "R" => Some(HQMTeam::Red),
                                    "b" | "B" => Some(HQMTeam::Blue),
                                    _ => None,
                                };
                                let round = args[2].parse::<u32>();
                                if let (Some(team), Ok(round)) = (team, round) {
                                    self.set_round(server, team, round, player_index);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            "redo" => {
                let args = arg.split(" ").collect::<Vec<&str>>();
                if args.len() >= 2 {
                    let team = match args[0] {
                        "r" | "R" => Some(HQMTeam::Red),
                        "b" | "B" => Some(HQMTeam::Blue),
                        _ => None,
                    };
                    let round = args[1].parse::<u32>();
                    if let (Some(team), Ok(round)) = (team, round) {
                        self.redo_round(server, team, round, player_index);
                    }
                }
            }
            "pause" | "pausegame" => {
                self.pause(server, player_index);
            }
            "unpause" | "unpausegame" => {
                self.unpause(server, player_index);
            }
            _ => {}
        }
    }

    fn create_game(&mut self) -> HQMGame {
        self.paused = false;
        self.status = HQMShootoutStatus::WaitingForGame;
        let mut game = HQMGame::new(1, self.physics_config.clone(), -10.0);

        game.time = 1000;
        game
    }

    fn before_player_exit(&mut self, _server: &mut HQMServer, player_index: HQMServerPlayerIndex) {
        self.team_switch_timer.remove(&player_index);
    }

    fn get_number_of_players(&self) -> u32 {
        self.team_max as u32
    }
}

fn find_empty_dual_control(
    server: &HQMServer,
    team: HQMTeam,
) -> Option<(
    HQMServerPlayerIndex,
    Option<HQMServerPlayerIndex>,
    Option<HQMServerPlayerIndex>,
)> {
    for (i, player) in server.players.iter() {
        if let Some(player) = player {
            if let HQMServerPlayerData::DualControl { movement, stick } = player.data {
                if movement.is_none() || stick.is_none() {
                    if let Some((_, dual_control_team)) = player.object {
                        if dual_control_team == team {
                            return Some((i, movement, stick));
                        }
                    }
                }
            }
        }
    }
    None
}

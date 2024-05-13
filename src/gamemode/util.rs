use crate::game::{PlayerIndex, Rink, Team};
use crate::gamemode::ServerStateMut;
use nalgebra::{Point3, Rotation3};
use std::collections::{HashMap, HashSet};
use std::f32::consts::{FRAC_PI_2, PI};
use tracing::info;

pub fn add_players<
    F1: Fn(Team, usize) -> (Point3<f32>, Rotation3<f32>),
    FSpectate: FnMut(PlayerIndex) -> (),
    FJoin: FnMut(PlayerIndex, Team) -> (),
>(
    mut server: ServerStateMut,
    team_max: usize,
    team_switch_timer: &mut HashMap<PlayerIndex, u32>,
    show_extra_messages: Option<&HashSet<PlayerIndex>>,
    coords: F1,
    mut on_spectate: FSpectate,
    mut on_join: FJoin,
) -> (usize, usize) {
    let mut red_player_count = 0;
    let mut blue_player_count = 0;
    let mut spectating_players = vec![];
    let mut joining_red = vec![];
    let mut joining_blue = vec![];
    for player in server.players().iter() {
        let player_index = player.index;
        let input = player.input();
        let team = player.team();
        team_switch_timer
            .get_mut(&player_index)
            .map(|x| *x = x.saturating_sub(1));
        if let Some(team) = team {
            if input.spectate() {
                team_switch_timer.insert(player_index, 500);
                spectating_players.push((player_index, player.name()))
            } else if team == Team::Red {
                red_player_count += 1;
            } else {
                blue_player_count += 1;
            }
        } else {
            if (input.join_red() || input.join_blue())
                && team_switch_timer
                    .get(&player_index)
                    .map_or(true, |x| *x == 0)
            {
                if input.join_red() {
                    joining_red.push((player_index, player.name()));
                } else if input.join_blue() {
                    joining_blue.push((player_index, player.name()));
                }
            }
        }
    }
    for (player_index, player_name) in spectating_players {
        info!("{} ({}) is spectating", player_name, player_index);
        server.move_to_spectator(player_index);
        on_spectate(player_index);
        if let Some(show_extra_messages) = show_extra_messages {
            let s = format!("{} is spectating", player_name);
            for i in show_extra_messages.iter() {
                server.add_directed_server_chat_message(s.clone(), *i);
            }
        }
    }

    let mut add_player = |i: usize,
                          player_index: PlayerIndex,
                          player_name: &str,
                          team: Team,
                          player_count: &mut usize|
     -> bool {
        if *player_count >= team_max {
            return false;
        }
        let (pos, rot) = coords(team, i);

        let res = server.spawn_skater(player_index, team, pos, rot, false);

        if res {
            info!(
                "{} ({}) has joined team {:?}",
                player_name, player_index, team
            );
            *player_count += 1;
            on_join(player_index, team);
            if let Some(show_extra_messages) = show_extra_messages {
                let s = format!("{} is playing for Red", player_name);
                for i in show_extra_messages.iter() {
                    server.add_directed_server_chat_message(s.clone(), *i);
                }
            }
        }
        res
    };

    for (i, (player_index, player_name)) in joining_red.into_iter().enumerate() {
        if !add_player(
            i,
            player_index,
            &player_name,
            Team::Red,
            &mut red_player_count,
        ) {
            break;
        }
    }
    for (i, (player_index, player_name)) in joining_blue.into_iter().enumerate() {
        if !add_player(
            i,
            player_index,
            &player_name,
            Team::Blue,
            &mut blue_player_count,
        ) {
            break;
        }
    }

    (red_player_count, blue_player_count)
}

#[derive(Eq, PartialEq, Debug, Copy, Clone)]
pub enum SpawnPoint {
    Center,
    Bench,
}

pub fn get_spawnpoint(
    rink: &Rink,
    team: Team,
    spawn_point: SpawnPoint,
) -> (Point3<f32>, Rotation3<f32>) {
    match team {
        Team::Red => match spawn_point {
            SpawnPoint::Center => {
                let (z, rot) = ((rink.length / 2.0) + 3.0, 0.0);
                let pos = Point3::new(rink.width / 2.0, 2.0, z);
                let rot = Rotation3::from_euler_angles(0.0, rot, 0.0);
                (pos, rot)
            }
            SpawnPoint::Bench => {
                let z = (rink.length / 2.0) + 4.0;
                let pos = Point3::new(0.5, 2.0, z);
                let rot = Rotation3::from_euler_angles(0.0, 3.0 * FRAC_PI_2, 0.0);
                (pos, rot)
            }
        },
        Team::Blue => match spawn_point {
            SpawnPoint::Center => {
                let (z, rot) = ((rink.length / 2.0) - 3.0, PI);
                let pos = Point3::new(rink.width / 2.0, 2.0, z);
                let rot = Rotation3::from_euler_angles(0.0, rot, 0.0);
                (pos, rot)
            }
            SpawnPoint::Bench => {
                let z = (rink.length / 2.0) - 4.0;
                let pos = Point3::new(0.5, 2.0, z);
                let rot = Rotation3::from_euler_angles(0.0, 3.0 * FRAC_PI_2, 0.0);
                (pos, rot)
            }
        },
    }
}

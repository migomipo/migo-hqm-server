use crate::hqm_game::{
    HQMBody, HQMGameObject, HQMGameWorld, HQMObjectIndex, HQMPhysicsConfiguration, HQMPuck,
    HQMRink, HQMRinkNet, HQMSkater, HQMSkaterCollisionBall, HQMSkaterHand, HQMTeam, LinesAndNet,
};
use nalgebra::{Point3, Rotation3, Vector2, Vector3};
use smallvec::SmallVec;
use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, FRAC_PI_8, PI};
use std::iter::FromIterator;

enum HQMCollision {
    PlayerRink((usize, usize), f32, Vector3<f32>),
    PlayerPlayer((usize, usize), (usize, usize), f32, Vector3<f32>),
}

#[derive(Debug, Copy, Clone)]
pub enum HQMSimulationEvent {
    PuckTouch {
        player: HQMObjectIndex,
        puck: HQMObjectIndex,
    },
    PuckReachedDefensiveLine {
        team: HQMTeam,
        puck: HQMObjectIndex,
    },
    PuckPassedDefensiveLine {
        team: HQMTeam,
        puck: HQMObjectIndex,
    },
    PuckReachedCenterLine {
        team: HQMTeam,
        puck: HQMObjectIndex,
    },
    PuckPassedCenterLine {
        team: HQMTeam,
        puck: HQMObjectIndex,
    },
    PuckReachedOffensiveZone {
        team: HQMTeam,
        puck: HQMObjectIndex,
    },
    PuckEnteredOffensiveZone {
        team: HQMTeam,
        puck: HQMObjectIndex,
    },

    PuckEnteredNet {
        team: HQMTeam,
        puck: HQMObjectIndex,
    },
    PuckPassedGoalLine {
        team: HQMTeam,
        puck: HQMObjectIndex,
    },
    PuckTouchedNet {
        team: HQMTeam,
        puck: HQMObjectIndex,
    },
}

fn replace_nan(v: f32, d: f32) -> f32 {
    if v.is_nan() {
        d
    } else {
        v
    }
}

type SimulationList = SmallVec<[HQMSimulationEvent; 16]>;
type CollisionList = SmallVec<[HQMCollision; 32]>;

impl HQMGameWorld {
    pub(crate) fn simulate_step(&mut self) -> SimulationList {
        let mut events: SimulationList = smallvec::SmallVec::new();
        let mut players: SmallVec<[(usize, &mut HQMSkater); 32]> = smallvec::SmallVec::new();
        let mut pucks: SmallVec<[(usize, &mut HQMPuck); 32]> = smallvec::SmallVec::new();
        for (i, o) in self.objects.objects.iter_mut().enumerate() {
            match o {
                HQMGameObject::Player(player) => players.push((i, player)),
                HQMGameObject::Puck(puck) => pucks.push((i, puck)),
                _ => {}
            }
        }

        let mut collisions: CollisionList = smallvec::SmallVec::new();
        for (i, (_, player)) in players.iter_mut().enumerate() {
            update_player(i, player, &self.physics_config, &self.rink, &mut collisions);
        }

        for i in 0..players.len() {
            for j in i + 1..players.len() {
                let (a, b) = players.split_at_mut(j);
                let (_, p1) = &mut a[i];
                let (_, p2) = &mut b[0];
                for (ib, p1_collision_ball) in p1.collision_balls.iter().enumerate() {
                    for (jb, p2_collision_ball) in p2.collision_balls.iter().enumerate() {
                        let pos_diff = &p1_collision_ball.pos - &p2_collision_ball.pos;
                        let radius_sum = &p1_collision_ball.radius + &p2_collision_ball.radius;
                        if pos_diff.norm() < radius_sum {
                            let overlap = radius_sum - pos_diff.norm();

                            collisions.push(HQMCollision::PlayerPlayer(
                                (i, ib),
                                (j, jb),
                                overlap,
                                pos_diff.normalize(),
                            ));
                        }
                    }
                }
                let stick_v = &p1.stick_pos - &p2.stick_pos;
                let stick_distance = stick_v.norm();
                if stick_distance < 0.25 {
                    let stick_overlap = 0.25 - stick_distance;
                    let normal = stick_v.normalize();
                    let mut force = normal.scale(0.125 * stick_overlap)
                        + (&p2.stick_velocity - &p1.stick_velocity).scale(0.25);
                    if force.dot(&normal) > 0.0 {
                        limit_friction(&mut force, &normal, 0.01);
                        p1.stick_velocity += force.scale(0.5);
                        p2.stick_velocity -= force.scale(0.5);
                    }
                }
            }
        }

        let pucks_old_pos: SmallVec<[Point3<f32>; 32]> =
            pucks.iter().map(|x| x.1.body.pos.clone()).collect();

        for (_, puck) in pucks.iter_mut() {
            puck.body.linear_velocity[1] -= self.physics_config.gravity;
        }

        update_sticks_and_pucks(
            &mut players,
            &mut pucks,
            &self.rink,
            &mut events,
            &self.physics_config,
        );

        for ((puck_index, puck), old_puck_pos) in pucks.iter_mut().zip(pucks_old_pos.iter()) {
            if puck.body.linear_velocity.norm() > 1.0 / 65536.0 {
                let scale = puck.body.linear_velocity.norm().powi(2) * 0.125 * 0.125;
                let scaled = puck.body.linear_velocity.normalize().scale(scale);
                puck.body.linear_velocity -= scaled;
            }
            if puck.body.angular_velocity.norm() > 1.0 / 65536.0 {
                rotate_matrix_around_axis(
                    &mut puck.body.rot,
                    &puck.body.angular_velocity.normalize(),
                    puck.body.angular_velocity.norm(),
                )
            }

            puck_detection(
                puck,
                *puck_index,
                &old_puck_pos,
                HQMTeam::Red,
                &self.rink.red_lines_and_net,
                &mut events,
            );
            puck_detection(
                puck,
                *puck_index,
                &old_puck_pos,
                HQMTeam::Blue,
                &self.rink.blue_lines_and_net,
                &mut events,
            );
        }

        apply_collisions(&mut players, &collisions);
        events
    }
}

fn update_sticks_and_pucks(
    players: &mut [(usize, &mut HQMSkater)],
    pucks: &mut [(usize, &mut HQMPuck)],
    rink: &HQMRink,
    events: &mut SimulationList,
    physics_config: &HQMPhysicsConfiguration,
) {
    for i in 0..10 {
        for (_, player) in players.iter_mut() {
            player.stick_pos += player.stick_velocity.scale(0.1);
        }
        for (puck_index, puck) in pucks.iter_mut() {
            let puck_index = HQMObjectIndex(*puck_index);
            puck.body.pos += puck.body.linear_velocity.scale(0.1);

            let puck_linear_velocity_before = puck.body.linear_velocity.clone_owned();
            let puck_angular_velocity_before = puck.body.angular_velocity.clone_owned();
            let puck_vertices = puck.get_puck_vertices();
            if i == 0 {
                do_puck_rink_forces(
                    puck,
                    &puck_vertices,
                    rink,
                    &puck_linear_velocity_before,
                    &puck_angular_velocity_before,
                    physics_config.puck_rink_friction,
                );
            }
            for (player_index, player) in players.iter_mut() {
                let player_index = HQMObjectIndex(*player_index);
                let old_stick_velocity = player.stick_velocity.clone_owned();
                if (&puck.body.pos - &player.stick_pos).norm() < 1.0 {
                    let has_touched = do_puck_stick_forces(
                        puck,
                        player,
                        &puck_vertices,
                        &puck_linear_velocity_before,
                        &puck_angular_velocity_before,
                        &old_stick_velocity,
                    );
                    if has_touched {
                        events.push(HQMSimulationEvent::PuckTouch {
                            puck: puck_index,
                            player: player_index,
                        })
                    }
                }
            }
            let red_net_collision = do_puck_post_forces(
                puck,
                &rink.red_lines_and_net.net,
                &puck_linear_velocity_before,
                &puck_angular_velocity_before,
            );
            let blue_net_collision = do_puck_post_forces(
                puck,
                &rink.blue_lines_and_net.net,
                &puck_linear_velocity_before,
                &puck_angular_velocity_before,
            );

            let red_net_collision = red_net_collision
                | do_puck_net_forces(
                    puck,
                    &rink.red_lines_and_net.net,
                    &puck_linear_velocity_before,
                    &puck_angular_velocity_before,
                );
            let blue_net_collision = blue_net_collision
                | do_puck_net_forces(
                    puck,
                    &rink.blue_lines_and_net.net,
                    &puck_linear_velocity_before,
                    &puck_angular_velocity_before,
                );

            if red_net_collision {
                events.push(HQMSimulationEvent::PuckTouchedNet {
                    team: HQMTeam::Red,
                    puck: puck_index,
                })
            }
            if blue_net_collision {
                events.push(HQMSimulationEvent::PuckTouchedNet {
                    team: HQMTeam::Blue,
                    puck: puck_index,
                })
            }
        }
    }
}

fn update_stick(
    player: &mut HQMSkater,
    linear_velocity_before: &Vector3<f32>,
    angular_velocity_before: &Vector3<f32>,
    rink: &HQMRink,
) {
    let stick_input = Vector2::new(
        replace_nan(player.input.stick[0], 0.0).clamp(-FRAC_PI_2, FRAC_PI_2),
        replace_nan(player.input.stick[1], 0.0).clamp(-5.0 * PI / 16.0, FRAC_PI_8),
    );

    let placement_diff = stick_input - &player.stick_placement;
    let placement_change = placement_diff.scale(0.0625) - player.stick_placement_delta.scale(0.5);
    let placement_change = limit_vector_length2(&placement_change, 0.0088888891);

    player.stick_placement_delta += placement_change;
    player.stick_placement += &player.stick_placement_delta;

    // Now that stick placement has been calculated,
    // we will use it to calculate the stick position and rotation

    let mul = match player.hand {
        HQMSkaterHand::Right => 1.0,
        HQMSkaterHand::Left => -1.0,
    };
    player.stick_rot = {
        let pivot1_pos =
            &player.body.pos + (&player.body.rot * Vector3::new(-0.375 * mul, -0.5, -0.125));

        let stick_pos_converted = player.body.rot.transpose() * (&player.stick_pos - pivot1_pos);

        let current_azimuth = stick_pos_converted[0].atan2(-stick_pos_converted[2]);
        let current_inclination = -stick_pos_converted[1]
            .atan2((stick_pos_converted[0].powi(2) + stick_pos_converted[2].powi(2)).sqrt());

        let mut new_stick_rotation = player.body.rot.clone();
        rotate_matrix_spherical(
            &mut new_stick_rotation,
            current_azimuth,
            current_inclination,
        );

        if player.stick_placement[1] > 0.0 {
            let axis = &new_stick_rotation * Vector3::y();
            rotate_matrix_around_axis(
                &mut new_stick_rotation,
                &axis,
                player.stick_placement[1] * mul * FRAC_PI_2,
            )
        }

        // Rotate around the stick axis
        let handle_axis = (&new_stick_rotation * Vector3::new(0.0, 0.75, 1.0)).normalize();
        rotate_matrix_around_axis(
            &mut new_stick_rotation,
            &handle_axis,
            (-replace_nan(player.input.stick_angle, 0.0)).clamp(-1.0, 1.0) * FRAC_PI_4,
        );

        new_stick_rotation
    };

    let (stick_force, intended_stick_position) = {
        let mut stick_rotation2 = player.body.rot.clone();
        rotate_matrix_spherical(
            &mut stick_rotation2,
            player.stick_placement[0],
            player.stick_placement[1],
        );

        let temp = stick_rotation2 * Vector3::x();
        rotate_matrix_around_axis(&mut stick_rotation2, &temp, FRAC_PI_4);

        let stick_length = 1.75;

        let stick_top_position =
            &player.body.pos + (&player.body.rot * Vector3::new(-0.375 * mul, 0.5, -0.125));
        let mut intended_stick_position =
            stick_top_position + (&stick_rotation2 * Vector3::z().scale(-stick_length));
        if intended_stick_position[1] < 0.0 {
            intended_stick_position[1] = 0.0;
        }

        let speed_at_stick_pos = speed_of_point_including_rotation(
            &intended_stick_position,
            &player.body.pos,
            linear_velocity_before,
            angular_velocity_before,
        );
        let stick_force = 0.125 * (intended_stick_position - &player.stick_pos)
            + (speed_at_stick_pos - &player.stick_velocity).scale(0.5);

        (stick_force, intended_stick_position)
    };

    player.stick_velocity += stick_force.scale(0.996);
    apply_acceleration_to_object(
        &mut player.body,
        &stick_force.scale(-0.004),
        &intended_stick_position,
    );

    if let Some((overlap, normal)) =
        collision_between_sphere_and_rink(&player.stick_pos, 0.09375, rink)
    {
        let mut n = normal.scale(overlap * 0.25) - player.stick_velocity.scale(0.5);
        if n.dot(&normal) > 0.0 {
            limit_friction(&mut n, &normal, 0.1);
            player.stick_velocity += n;
        }
    }
}

fn update_player(
    i: usize,
    player: &mut HQMSkater,
    physics_config: &HQMPhysicsConfiguration,
    rink: &HQMRink,
    collisions: &mut CollisionList,
) {
    let linear_velocity_before = player.body.linear_velocity.clone_owned();
    let angular_velocity_before = player.body.angular_velocity.clone_owned();

    player.body.pos += &player.body.linear_velocity;
    player.body.linear_velocity[1] -= physics_config.gravity;
    for collision_ball in player.collision_balls.iter_mut() {
        collision_ball.velocity *= 0.999;
        collision_ball.pos += &collision_ball.velocity;
        collision_ball.velocity[1] -= physics_config.gravity;
    }
    let feet_pos = &player.body.pos - (&player.body.rot * Vector3::y().scale(player.height));
    if feet_pos[1] < 0.0 {
        let fwbw_from_client = player.input.fwbw.clamp(-1.0, 1.0);
        if fwbw_from_client != 0.0 {
            let mut skate_direction = if fwbw_from_client > 0.0 {
                &player.body.rot * -Vector3::z()
            } else {
                &player.body.rot * Vector3::z()
            };
            let max_acceleration = if player.body.linear_velocity.dot(&skate_direction) < 0.0 {
                physics_config.player_deceleration // If we're accelerating against the current direction of movement
                                                   // we're decelerating and can do so faster
            } else {
                physics_config.player_acceleration
            };
            skate_direction[1] = 0.0;
            skate_direction.normalize_mut();
            let new_acceleration = skate_direction.scale(physics_config.max_player_speed)
                - &player.body.linear_velocity;

            player.body.linear_velocity += limit_vector_length(&new_acceleration, max_acceleration);
        }
        if player.input.jump() && !player.jumped_last_frame {
            let diff = if physics_config.limit_jump_speed {
                (0.025 - player.body.linear_velocity[1]).clamp(0.0, 0.025)
            } else {
                0.025
            };
            if diff != 0.0 {
                player.body.linear_velocity[1] += diff;
                for collision_ball in player.collision_balls.iter_mut() {
                    collision_ball.velocity[1] += diff;
                }
            }
        }
    }
    player.jumped_last_frame = player.input.jump();

    // Turn player
    let turn = player.input.turn.clamp(-1.0, 1.0);
    let mut turn_change = &player.body.rot * Vector3::y();
    if player.input.shift() {
        let mut velocity_adjustment = &player.body.rot * Vector3::x();
        velocity_adjustment[1] = 0.0;
        velocity_adjustment.normalize_mut();
        velocity_adjustment.scale_mut(physics_config.max_player_shift_speed * turn);
        velocity_adjustment -= &player.body.linear_velocity;
        player.body.linear_velocity += limit_vector_length(
            &velocity_adjustment,
            physics_config.player_shift_acceleration,
        );
        turn_change.scale_mut(-turn * physics_config.player_shift_turning);
        player.body.angular_velocity += turn_change;
    } else {
        turn_change.scale_mut(turn * physics_config.player_turning);
        player.body.angular_velocity += turn_change;
    }

    if player.body.angular_velocity.norm() > 1.0 / 65536.0 {
        rotate_matrix_around_axis(
            &mut player.body.rot,
            &player.body.angular_velocity.normalize(),
            player.body.angular_velocity.norm(),
        );
    }
    adjust_head_body_rot(
        &mut player.head_rot,
        player
            .input
            .head_rot
            .clamp(-7.0 * FRAC_PI_8, 7.0 * FRAC_PI_8),
    );
    adjust_head_body_rot(
        &mut player.body_rot,
        player.input.body_rot.clamp(-FRAC_PI_2, FRAC_PI_2),
    );
    for (collision_ball_index, collision_ball) in player.collision_balls.iter_mut().enumerate() {
        let mut new_rot = player.body.rot.clone();
        if collision_ball_index == 1 || collision_ball_index == 2 || collision_ball_index == 5 {
            let rot_axis = &new_rot * Vector3::y();
            rotate_matrix_around_axis(&mut new_rot, &rot_axis, player.head_rot * 0.5);
            let rot_axis = &new_rot * Vector3::x();
            rotate_matrix_around_axis(&mut new_rot, &rot_axis, player.body_rot);
        }
        let intended_collision_ball_pos = &player.body.pos + (new_rot * &collision_ball.offset);
        let collision_pos_diff = intended_collision_ball_pos - &collision_ball.pos;

        let speed = speed_of_point_including_rotation(
            &intended_collision_ball_pos,
            &player.body.pos,
            &linear_velocity_before,
            &angular_velocity_before,
        );
        let force =
            collision_pos_diff.scale(0.125) + (speed - &collision_ball.velocity).scale(0.25);
        collision_ball.velocity += force.scale(0.9375);
        apply_acceleration_to_object(
            &mut player.body,
            &force.scale(0.9375 - 1.0),
            &intended_collision_ball_pos,
        );
    }

    for (ib, collision_ball) in player.collision_balls.iter().enumerate() {
        let collision = collision_between_collision_ball_and_rink(collision_ball, rink);
        if let Some((overlap, normal)) = collision {
            collisions.push(HQMCollision::PlayerRink((i, ib), overlap, normal));
        }
    }
    let linear_velocity_before = player.body.linear_velocity.clone_owned();
    let angular_velocity_before = player.body.angular_velocity.clone_owned();

    if player.input.crouch() {
        player.height = (player.height - 0.015625).max(0.25)
    } else {
        player.height = (player.height + 0.125).min(0.75);
    }

    let feet_pos = &player.body.pos - &player.body.rot * Vector3::y().scale(player.height);
    let mut touches_ice = false;
    if feet_pos[1] < 0.0 {
        // Makes players bounce up if their feet get below the ice
        let temp1 = -feet_pos[1] * 0.125 * 0.125 * 0.25;
        let unit_y = Vector3::y();

        let mut temp2 = unit_y.scale(temp1) - player.body.linear_velocity.scale(0.25);
        if temp2.dot(&unit_y) > 0.0 {
            let (column, rejection_limit) = if player.input.shift() {
                (Vector3::x(), 0.4)
            } else {
                (Vector3::z(), 1.2)
            };
            let mut direction = &player.body.rot * column;
            direction[1] = 0.0;

            temp2 -= get_projection(&temp2, &direction);

            limit_friction(&mut temp2, &unit_y, rejection_limit);
            player.body.linear_velocity += temp2;
            touches_ice = true;
        }
    }
    if player.body.pos[1] < 0.5 && player.body.linear_velocity.norm() < 0.025 {
        player.body.linear_velocity[1] += 0.00055555555;
        touches_ice = true;
    }
    if touches_ice {
        // This is where the leaning happens
        player.body.angular_velocity.scale_mut(0.975);
        let mut unit: Vector3<f32> = Vector3::y();

        if !player.input.shift() {
            let axis = &player.body.rot * Vector3::z();
            let temp = -player.body.linear_velocity.dot(&axis) / physics_config.max_player_speed;
            rotate_vector_around_axis(&mut unit, &axis, 0.225 * turn * temp);
        }

        let temp2 = unit.cross(&(&player.body.rot * Vector3::y()));

        let temp2 = temp2.scale(0.008333333)
            - get_projection(&player.body.angular_velocity, &temp2).scale(0.25);
        let temp2 = limit_vector_length(&temp2, 0.000347222222);
        player.body.angular_velocity += temp2;
    }
    update_stick(
        player,
        &linear_velocity_before,
        &angular_velocity_before,
        rink,
    );
}

// Project a onto b
fn get_projection(a: &Vector3<f32>, b: &Vector3<f32>) -> Vector3<f32> {
    let normal = normal_or_zero(b);
    normal.scale(normal.dot(&a))
}

fn apply_collisions(players: &mut [(usize, &mut HQMSkater)], collisions: &[HQMCollision]) {
    for _ in 0..16 {
        let original_ball_velocities =
            SmallVec::<[_; 32]>::from_iter(players.iter().map(|(_, skater)| {
                SmallVec::<[_; 8]>::from_iter(
                    skater
                        .collision_balls
                        .iter()
                        .map(|x| x.velocity.clone_owned()),
                )
            }));

        for collision_event in collisions.iter() {
            match collision_event {
                HQMCollision::PlayerRink((i2, ib2), overlap, normal) => {
                    let (i, ib) = (*i2, *ib2);

                    let original_velocity = &original_ball_velocities[i][ib];
                    let mut new = normal.scale(overlap * 0.03125) - original_velocity.scale(0.25);
                    if new.dot(&normal) > 0.0 {
                        limit_friction(&mut new, &normal, 0.01);
                        let (_, skater) = &mut players[i];
                        let ball = &mut skater.collision_balls[ib];
                        ball.velocity += new;
                    }
                }
                HQMCollision::PlayerPlayer((i2, ib2), (j2, jb2), overlap, normal) => {
                    let (i, ib) = (*i2, *ib2);
                    let (j, jb) = (*j2, *jb2);
                    let original_velocity1 = &original_ball_velocities[i][ib];
                    let original_velocity2 = &original_ball_velocities[j][jb];

                    let mut new = normal.scale(overlap * 0.125)
                        + (original_velocity2 - original_velocity1).scale(0.25);
                    if new.dot(&normal) > 0.0 {
                        limit_friction(&mut new, &normal, 0.01);
                        let (_, skater1) = &players[i];
                        let (_, skater2) = &players[j];
                        let mass1 = skater1.collision_balls[ib].mass;
                        let mass2 = skater2.collision_balls[jb].mass;
                        let mass_sum = mass1 + mass2;

                        let (_, skater1) = &mut players[i];
                        skater1.collision_balls[ib].velocity += new.scale(mass2 / mass_sum);

                        let (_, skater2) = &mut players[j];
                        skater2.collision_balls[jb].velocity -= new.scale(mass1 / mass_sum);
                    }
                }
            }
        }
    }
}

fn puck_detection(
    puck: &mut HQMPuck,
    puck_index: usize,
    old_puck_pos: &Point3<f32>,
    team: HQMTeam,
    lines_and_net: &LinesAndNet,
    events: &mut SimulationList,
) {
    let puck_index = HQMObjectIndex(puck_index);
    let offensive_line = &lines_and_net.offensive_line;
    let mid_line = &lines_and_net.mid_line;
    let defensive_line = &lines_and_net.defensive_line;
    let net = &lines_and_net.net;

    if defensive_line.sphere_reached_line(&puck.body.pos, puck.radius)
        && !defensive_line.sphere_reached_line(&old_puck_pos, puck.radius)
    {
        let event = HQMSimulationEvent::PuckReachedDefensiveLine {
            team,
            puck: puck_index,
        };
        events.push(event);
    }
    if defensive_line.sphere_past_leading_edge(&puck.body.pos, puck.radius)
        && !defensive_line.sphere_past_leading_edge(&old_puck_pos, puck.radius)
    {
        let event = HQMSimulationEvent::PuckPassedDefensiveLine {
            team,
            puck: puck_index,
        };
        events.push(event);
    }

    if mid_line.sphere_reached_line(&puck.body.pos, puck.radius)
        && !mid_line.sphere_reached_line(&old_puck_pos, puck.radius)
    {
        let event = HQMSimulationEvent::PuckReachedCenterLine {
            team,
            puck: puck_index,
        };
        events.push(event);
    }
    if mid_line.sphere_past_leading_edge(&puck.body.pos, puck.radius)
        && !mid_line.sphere_past_leading_edge(&old_puck_pos, puck.radius)
    {
        let event = HQMSimulationEvent::PuckPassedCenterLine {
            team,
            puck: puck_index,
        };
        events.push(event);
    }
    if offensive_line.sphere_reached_line(&puck.body.pos, puck.radius)
        && !offensive_line.sphere_reached_line(old_puck_pos, puck.radius)
    {
        let event = HQMSimulationEvent::PuckReachedOffensiveZone {
            team,
            puck: puck_index,
        };
        events.push(event);
    }
    if offensive_line.sphere_past_leading_edge(&puck.body.pos, puck.radius)
        && !offensive_line.sphere_past_leading_edge(old_puck_pos, puck.radius)
    {
        let event = HQMSimulationEvent::PuckEnteredOffensiveZone {
            team,
            puck: puck_index,
        };
        events.push(event);
    }
    if (&net.left_post - &puck.body.pos).dot(&net.normal) >= 0.0 {
        if (&net.left_post - old_puck_pos).dot(&net.normal) < 0.0 {
            if (&net.left_post - &puck.body.pos).dot(&net.left_post_inside) < 0.0
                && (&net.right_post - &puck.body.pos).dot(&net.right_post_inside) < 0.0
                && puck.body.pos.y < 1.0
            {
                let event = HQMSimulationEvent::PuckEnteredNet {
                    team,
                    puck: puck_index,
                };
                events.push(event);
            } else {
                let event = HQMSimulationEvent::PuckPassedGoalLine {
                    team,
                    puck: puck_index,
                };
                events.push(event);
            }
        }
    }
}

fn do_puck_net_forces(
    puck: &mut HQMPuck,
    net: &HQMRinkNet,
    puck_linear_velocity: &Vector3<f32>,
    puck_angular_velocity: &Vector3<f32>,
) -> bool {
    let mut res = false;
    if let Some((overlap_pos, overlap, normal)) =
        collision_between_sphere_and_net(&puck.body.pos, puck.radius, net)
    {
        res = true;
        let vertex_velocity = speed_of_point_including_rotation(
            &overlap_pos,
            &puck.body.pos,
            &puck_linear_velocity,
            &puck_angular_velocity,
        );
        let mut puck_force = normal.scale(overlap * 0.5) - vertex_velocity.scale(0.5);

        if normal.dot(&puck_force) > 0.0 {
            limit_friction(&mut puck_force, &normal, 0.5);
            apply_acceleration_to_object(&mut puck.body, &puck_force, &overlap_pos);
            puck.body.linear_velocity *= 0.9875;
            puck.body.angular_velocity *= 0.95;
        }
    }
    res
}

fn do_puck_post_forces(
    puck: &mut HQMPuck,
    net: &HQMRinkNet,
    puck_linear_velocity: &Vector3<f32>,
    puck_angular_velocity: &Vector3<f32>,
) -> bool {
    let mut res = false;
    for post in net.posts.iter() {
        let collision = collision_between_sphere_and_post(&puck.body.pos, puck.radius, post);
        if let Some((overlap, normal)) = collision {
            res = true;
            let p = &puck.body.pos - puck.radius * normal;
            let vertex_velocity = speed_of_point_including_rotation(
                &p,
                &puck.body.pos,
                puck_linear_velocity,
                puck_angular_velocity,
            );
            let mut puck_force = normal.scale(overlap * 0.125) - vertex_velocity.scale(0.25);

            if normal.dot(&puck_force) > 0.0 {
                limit_friction(&mut puck_force, &normal, 0.2);
                apply_acceleration_to_object(&mut puck.body, &puck_force, &p);
            }
        }
    }
    res
}

fn do_puck_stick_forces(
    puck: &mut HQMPuck,
    player: &mut HQMSkater,
    puck_vertices: &[Point3<f32>],
    puck_linear_velocity: &Vector3<f32>,
    puck_angular_velocity: &Vector3<f32>,
    stick_velocity: &Vector3<f32>,
) -> bool {
    let stick_surfaces = get_stick_surfaces(player);
    let mut res = false;
    for puck_vertex in puck_vertices.iter() {
        let col =
            collision_between_puck_vertex_and_stick(&puck.body.pos, puck_vertex, &stick_surfaces);
        if let Some((dot, normal)) = col {
            res = true;
            let puck_vertex_speed = speed_of_point_including_rotation(
                &puck_vertex,
                &puck.body.pos,
                puck_linear_velocity,
                puck_angular_velocity,
            );

            let mut puck_force =
                normal.scale(dot * 0.125 * 0.5) + (stick_velocity - puck_vertex_speed).scale(0.125);
            if puck_force.dot(&normal) > 0.0 {
                limit_friction(&mut puck_force, &normal, 0.5);
                player.stick_velocity -= puck_force.scale(0.25);
                puck_force.scale_mut(0.75);
                apply_acceleration_to_object(&mut puck.body, &puck_force, &puck_vertex);
            }
        }
    }
    res
}

fn do_puck_rink_forces(
    puck: &mut HQMPuck,
    puck_vertices: &[Point3<f32>],
    rink: &HQMRink,
    puck_linear_velocity: &Vector3<f32>,
    puck_angular_velocity: &Vector3<f32>,
    friction: f32,
) {
    for vertex in puck_vertices.iter() {
        let c = collision_between_vertex_and_rink(vertex, rink);
        if let Some((overlap, normal)) = c {
            let vertex_velocity = speed_of_point_including_rotation(
                &vertex,
                &puck.body.pos,
                puck_linear_velocity,
                puck_angular_velocity,
            );
            let mut puck_force =
                (normal.scale(overlap * 0.5) - vertex_velocity).scale(0.125 * 0.125);

            if normal.dot(&puck_force) > 0.0 {
                limit_friction(&mut puck_force, &normal, friction);
                apply_acceleration_to_object(&mut puck.body, &puck_force, &vertex);
            }
        }
    }
}

fn get_stick_surfaces(
    player: &HQMSkater,
) -> [(Point3<f32>, Point3<f32>, Point3<f32>, Point3<f32>); 6] {
    let stick_size = Vector3::new(0.0625, 0.25, 0.5);
    let nnn = &player.stick_pos
        + &player.stick_rot * Vector3::new(-0.5, -0.5, -0.5).component_mul(&stick_size);
    let nnp = &player.stick_pos
        + &player.stick_rot * Vector3::new(-0.5, -0.5, 0.5).component_mul(&stick_size);
    let npn = &player.stick_pos
        + &player.stick_rot * Vector3::new(-0.5, 0.5, -0.5).component_mul(&stick_size);
    let npp = &player.stick_pos
        + &player.stick_rot * Vector3::new(-0.5, 0.5, 0.5).component_mul(&stick_size);
    let pnn = &player.stick_pos
        + &player.stick_rot * Vector3::new(0.5, -0.5, -0.5).component_mul(&stick_size);
    let pnp = &player.stick_pos
        + &player.stick_rot * Vector3::new(0.5, -0.5, 0.5).component_mul(&stick_size);
    let ppn = &player.stick_pos
        + &player.stick_rot * Vector3::new(0.5, 0.5, -0.5).component_mul(&stick_size);
    let ppp = &player.stick_pos
        + &player.stick_rot * Vector3::new(0.5, 0.5, 0.5).component_mul(&stick_size);

    let res = [
        (nnp.clone(), pnp.clone(), pnn.clone(), nnn.clone()),
        (npp.clone(), ppp.clone(), pnp.clone(), nnp.clone()),
        (npn.clone(), npp.clone(), nnp.clone(), nnn.clone()),
        (ppn.clone(), npn.clone(), nnn.clone(), pnn.clone()),
        (ppp.clone(), ppn.clone(), pnn.clone(), pnp.clone()),
        (npn.clone(), ppn.clone(), ppp.clone(), npp.clone()),
    ];
    res
}

fn inside_surface(
    pos: &Point3<f32>,
    surface: &(Point3<f32>, Point3<f32>, Point3<f32>, Point3<f32>),
    normal: &Vector3<f32>,
) -> bool {
    let (p1, p2, p3, p4) = surface;
    (pos - p1).cross(&(p2 - p1)).dot(&normal) >= 0.0
        && (pos - p2).cross(&(p3 - p2)).dot(&normal) >= 0.0
        && (pos - p3).cross(&(p4 - p3)).dot(&normal) >= 0.0
        && (pos - p4).cross(&(p1 - p4)).dot(&normal) >= 0.0
}

fn collision_between_sphere_and_net(
    pos: &Point3<f32>,
    radius: f32,
    net: &HQMRinkNet,
) -> Option<(Point3<f32>, f32, Vector3<f32>)> {
    let mut max_overlap = 0.0;
    let mut res: Option<(Point3<f32>, f32, Vector3<f32>)> = None;

    for surface in net.surfaces.iter() {
        let normal = (&surface.3 - &surface.0)
            .cross(&(&surface.1 - &surface.0))
            .normalize();

        let diff = &surface.0 - pos;
        let dot = diff.dot(&normal);
        let overlap = dot + radius;
        let overlap2 = -dot + radius;

        if overlap > 0.0 && overlap < radius {
            let overlap_pos = pos + (radius - overlap) * normal;
            if inside_surface(&overlap_pos, surface, &normal) {
                if overlap > max_overlap {
                    max_overlap = overlap;
                    res = Some((overlap_pos, overlap, normal));
                }
            }
        } else if overlap2 > 0.0 && overlap2 < radius {
            let overlap_pos = pos + (radius - overlap) * normal;
            if inside_surface(&overlap_pos, surface, &normal) {
                if overlap2 > max_overlap {
                    max_overlap = overlap2;
                    res = Some((overlap_pos, overlap2, -normal));
                }
            }
        }
    }

    res
}

fn collision_between_sphere_and_post(
    pos: &Point3<f32>,
    radius: f32,
    post: &(Point3<f32>, Point3<f32>, f32),
) -> Option<(f32, Vector3<f32>)> {
    let (p1, p2, post_radius) = post;
    let a = post_radius + radius;
    let direction_vector = p2 - p1;

    let diff = pos - p1;
    let t0 = diff.dot(&direction_vector) / direction_vector.norm_squared();
    let dot = t0.clamp(0.0, 1.0);

    let projection = dot * &direction_vector;
    let rejection = diff - projection;
    let rejection_norm = rejection.norm();
    let overlap = a - rejection_norm;
    if overlap > 0.0 {
        Some((overlap, rejection.normalize()))
    } else {
        None
    }
}

fn collision_between_puck_and_surface(
    puck_pos: &Point3<f32>,
    puck_pos2: &Point3<f32>,
    surface: &(Point3<f32>, Point3<f32>, Point3<f32>, Point3<f32>),
) -> Option<(f32, Point3<f32>, f32, Vector3<f32>)> {
    let normal = (&surface.3 - &surface.0)
        .cross(&(&surface.1 - &surface.0))
        .normalize();
    let p1 = &surface.0;
    let puck_pos2_projection = (p1 - puck_pos2).dot(&normal);
    if puck_pos2_projection >= 0.0 {
        let puck_pos_projection = (p1 - puck_pos).dot(&normal);
        if puck_pos_projection <= 0.0 {
            let diff = puck_pos2 - puck_pos;
            let diff_projection = diff.dot(&normal);
            if diff_projection != 0.0 {
                let intersection = puck_pos_projection / diff_projection;
                let intersection_pos = puck_pos + diff.scale(intersection);

                let overlap = (&intersection_pos - puck_pos2).dot(&normal);

                if inside_surface(&intersection_pos, surface, &normal) {
                    return Some((intersection, intersection_pos, overlap, normal));
                }
            }
        }
    }
    None
}

fn collision_between_puck_vertex_and_stick(
    puck_pos: &Point3<f32>,
    puck_vertex: &Point3<f32>,
    stick_surfaces: &[(Point3<f32>, Point3<f32>, Point3<f32>, Point3<f32>)],
) -> Option<(f32, Vector3<f32>)> {
    let mut min_intersection = 1f32;
    let mut res = None;
    for stick_surface in stick_surfaces.iter() {
        let collision = collision_between_puck_and_surface(puck_pos, puck_vertex, stick_surface);
        if let Some((intersection, _intersection_pos, overlap, normal)) = collision {
            if intersection < min_intersection {
                res = Some((overlap, normal));
                min_intersection = intersection;
            }
        }
    }
    res
}

fn collision_between_sphere_and_rink(
    pos: &Point3<f32>,
    radius: f32,
    rink: &HQMRink,
) -> Option<(f32, Vector3<f32>)> {
    let mut max_overlap = 0f32;
    let mut coll_normal = None;
    for (p, normal) in rink.planes.iter() {
        let overlap = (p - pos).dot(normal) + radius;
        if overlap > max_overlap {
            max_overlap = overlap;
            coll_normal = Some(normal.clone_owned());
        }
    }
    for (p, dir, corner_radius) in rink.corners.iter() {
        let mut p2 = p - pos;
        p2[1] = 0.0;
        if p2[0] * dir[0] < 0.0 && p2[2] * dir[2] < 0.0 {
            let overlap = p2.norm() + radius - corner_radius;
            if overlap > max_overlap {
                max_overlap = overlap;
                let p2n = p2.normalize();
                coll_normal = Some(p2n);
            }
        }
    }
    match coll_normal {
        Some(n) => Some((max_overlap, n)),
        None => None,
    }
}

fn collision_between_collision_ball_and_rink(
    ball: &HQMSkaterCollisionBall,
    rink: &HQMRink,
) -> Option<(f32, Vector3<f32>)> {
    collision_between_sphere_and_rink(&ball.pos, ball.radius, rink)
}

fn collision_between_vertex_and_rink(
    vertex: &Point3<f32>,
    rink: &HQMRink,
) -> Option<(f32, Vector3<f32>)> {
    collision_between_sphere_and_rink(vertex, 0.0, rink)
}

fn apply_acceleration_to_object(body: &mut HQMBody, change: &Vector3<f32>, point: &Point3<f32>) {
    let diff1 = point - &body.pos;
    body.linear_velocity += change;
    let cross = change.cross(&diff1);
    body.angular_velocity +=
        &body.rot * (body.rot.transpose() * cross).component_mul(&body.rot_mul);
}

fn speed_of_point_including_rotation(
    p: &Point3<f32>,
    pos: &Point3<f32>,
    linear_velocity: &Vector3<f32>,
    angular_velocity: &Vector3<f32>,
) -> Vector3<f32> {
    linear_velocity + (p - pos).cross(angular_velocity)
}

fn rotate_matrix_spherical(matrix: &mut Rotation3<f32>, azimuth: f32, inclination: f32) {
    let col1 = &*matrix * Vector3::y();
    rotate_matrix_around_axis(matrix, &col1, azimuth);
    let col0 = &*matrix * Vector3::x();
    rotate_matrix_around_axis(matrix, &col0, inclination);
}

fn adjust_head_body_rot(rot: &mut f32, input_rot: f32) {
    let head_rot_diff = input_rot - *rot;
    if head_rot_diff <= 0.06666667 {
        if head_rot_diff >= -0.06666667 {
            *rot = input_rot;
        } else {
            *rot -= 0.06666667;
        }
    } else {
        *rot += 0.06666667;
    }
}

fn limit_vector_length(v: &Vector3<f32>, max_len: f32) -> Vector3<f32> {
    let norm = v.norm();
    let mut res = v.clone_owned();
    if norm > max_len {
        res.scale_mut(max_len / norm);
    }
    res
}

fn limit_vector_length2(v: &Vector2<f32>, max_len: f32) -> Vector2<f32> {
    let norm = v.norm();
    let mut res = v.clone_owned();
    if norm > max_len {
        res.scale_mut(max_len / norm);
    }
    res
}

pub fn limit_friction(v: &mut Vector3<f32>, normal: &Vector3<f32>, d: f32) {
    let projection_length = v.dot(&normal);
    let projection = normal.scale(projection_length);
    let rejection = &*v - &projection;
    let rejection_length = rejection.norm();
    *v = projection.clone_owned();

    if rejection_length > 1.0 / 65536.0 {
        let rejection_norm = rejection.normalize();

        let rejection_length2 = rejection_length.min(projection.norm() * d);
        *v += rejection_norm.scale(rejection_length2);
    }
}

fn rotate_vector_around_axis(v: &mut Vector3<f32>, axis: &Vector3<f32>, angle: f32) {
    let axis_angle = -angle * axis;
    let rot = Rotation3::new(axis_angle);
    *v = &rot * *v;
}

fn rotate_matrix_around_axis(v: &mut Rotation3<f32>, axis: &Vector3<f32>, angle: f32) {
    let axis_angle = -angle * axis;
    let rot = Rotation3::new(axis_angle);
    *v = rot * *v;
}

fn normal_or_zero(v: &Vector3<f32>) -> Vector3<f32> {
    if let Some(r) = v.try_normalize(0.0) {
        r
    } else {
        Vector3::new(0.0, 0.0, 0.0)
    }
}

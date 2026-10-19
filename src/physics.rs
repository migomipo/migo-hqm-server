use crate::game::RinkSideOfLine::{BlueSide, RedSide};
use crate::game::{
    PhysicsConfiguration, PlayerInput, Puck, Rink, RinkNet, SkaterCollisionBall, SkaterHand,
    SkaterObject, Team,
};
use crate::game::{PhysicsEvent, PlayerId, PuckId};
use crate::players::PlayerListExt;
use crate::server::{GameObject, HQMServer};
use arrayvec::ArrayVec;
use glam::{EulerRot, Vec2, Vec3};
use glamx::Rot3;
use smallvec::SmallVec;
use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, FRAC_PI_8, PI};
use std::iter::FromIterator;

enum Collision {
    PlayerRink((usize, usize), f32, Vec3),
    PlayerPlayer((usize, usize), (usize, usize), f32, Vec3),
}

fn replace_nan(v: f32, d: f32) -> f32 {
    if v.is_nan() { d } else { v }
}

type PhysicsEventList = SmallVec<[PhysicsEvent; 16]>;
type CollisionList = SmallVec<[Collision; 32]>;

impl HQMServer {
    pub(crate) fn simulate_step(&mut self) -> PhysicsEventList {
        let mut events: PhysicsEventList = SmallVec::new();
        let mut players: ArrayVec<(PlayerId, &mut SkaterObject, &PlayerInput), 32> =
            ArrayVec::new();
        let mut pucks: ArrayVec<(PuckId, &mut Puck, Vec3), 32> = ArrayVec::new();
        for object in self.state.objects.iter_mut().flatten() {
            match object {
                GameObject::Skater(player_id, skater) => {
                    let player = self
                        .state
                        .player_message_state
                        .players
                        .get_player(*player_id)
                        .unwrap();
                    players.push((*player_id, skater, &player.input))
                }
                GameObject::Puck(puck_id, puck) => {
                    let old_pos = puck.body.pos;
                    pucks.push((*puck_id, puck, old_pos));
                }
            }
        }

        let mut collisions: CollisionList = SmallVec::new();
        for (i, (_, player, input)) in players.iter_mut().enumerate() {
            update_player(
                i,
                player,
                input,
                &self.physics_config,
                &self.rink,
                &mut collisions,
            );
        }

        for i in 0..players.len() {
            let (a, b) = players.split_at_mut(i + 1);
            let (_, p1, _) = &mut a[i];

            for (j, (_, p2, _)) in ((i + 1)..).zip(b.iter_mut()) {
                for (ib, p1_collision_ball) in p1.collision_balls.iter().enumerate() {
                    for (jb, p2_collision_ball) in p2.collision_balls.iter().enumerate() {
                        let pos_diff = p1_collision_ball.pos - p2_collision_ball.pos;
                        let radius_sum = p1_collision_ball.radius + p2_collision_ball.radius;
                        let len = pos_diff.length();
                        if len < radius_sum {
                            collisions.push(Collision::PlayerPlayer(
                                (i, ib),
                                (j, jb),
                                radius_sum - len,
                                pos_diff / len, // normalize manually, avoids 4th sqrt inside normalize()
                            ));
                        }
                    }
                }
                let stick_v = p1.stick_pos - p2.stick_pos;
                let stick_distance = stick_v.length();
                if stick_distance < 0.25 {
                    let stick_overlap = 0.25 - stick_distance;
                    let normal = stick_v.normalize();
                    let mut force = 0.125 * stick_overlap * normal
                        + 0.25 * (p2.stick_velocity - p1.stick_velocity);
                    if force.dot(normal) > 0.0 {
                        limit_friction(&mut force, normal, 0.01);
                        let force = 0.5 * force;
                        p1.stick_velocity += 0.5 * force;
                        p2.stick_velocity -= 0.5 * force;
                    }
                }
            }
        }

        for (_, puck, _) in pucks.iter_mut() {
            puck.body.linear_velocity[1] -= self.physics_config.gravity;
        }

        update_sticks_and_pucks(
            &mut players,
            &mut pucks,
            &self.rink,
            &mut events,
            &self.physics_config,
        );

        for (puck_index, puck, old_puck_pos) in pucks.iter_mut() {
            if puck.body.linear_velocity.length() > 1.0 / 65536.0 {
                let scale = puck.body.linear_velocity.length().powi(2) * 0.125 * 0.125;
                let scaled = scale * puck.body.linear_velocity.normalize();
                puck.body.linear_velocity -= scaled;
            }
            if puck.body.angular_velocity.length() > 1.0 / 65536.0 {
                puck.body.rot = Rot3::from_scaled_axis(puck.body.angular_velocity) * puck.body.rot;
            }

            puck_detection(puck, *puck_index, *old_puck_pos, &self.rink, &mut events);
        }

        apply_collisions(&mut players, &collisions);
        events
    }
}

fn update_sticks_and_pucks(
    players: &mut [(PlayerId, &mut SkaterObject, &PlayerInput)],
    pucks: &mut [(PuckId, &mut Puck, Vec3)],
    rink: &Rink,
    events: &mut PhysicsEventList,
    physics_config: &PhysicsConfiguration,
) {
    for i in 0..10 {
        for (_, player, _) in players.iter_mut() {
            player.stick_pos += 0.1 * player.stick_velocity;
        }

        for (puck_index, puck, _) in pucks.iter_mut() {
            puck.body.pos += 0.1 * puck.body.linear_velocity;

            let puck_linear_velocity_before = puck.body.linear_velocity;
            let puck_angular_velocity_before = puck.body.angular_velocity;
            let puck_vertices = puck.get_puck_vertices();
            if i == 0 {
                do_puck_rink_forces(
                    puck,
                    &puck_vertices,
                    rink,
                    puck_linear_velocity_before,
                    puck_angular_velocity_before,
                    physics_config.puck_rink_friction,
                );
            }
            for (player_index, player, _) in players.iter_mut() {
                let old_stick_velocity = player.stick_velocity;
                if (puck.body.pos - player.stick_pos).length() < 1.0 {
                    let has_touched = do_puck_stick_forces(
                        puck,
                        player,
                        &puck_vertices,
                        puck_linear_velocity_before,
                        puck_angular_velocity_before,
                        old_stick_velocity,
                    );
                    if has_touched {
                        events.push(PhysicsEvent::PuckTouch {
                            puck: *puck_index,
                            player: *player_index,
                        })
                    }
                }
            }
            let red_net_collision = do_puck_post_forces(
                puck,
                &rink.red_net,
                puck_linear_velocity_before,
                puck_angular_velocity_before,
            );
            let blue_net_collision = do_puck_post_forces(
                puck,
                &rink.blue_net,
                puck_linear_velocity_before,
                puck_angular_velocity_before,
            );

            let red_net_collision = red_net_collision
                | do_puck_net_forces(
                    puck,
                    &rink.red_net,
                    puck_linear_velocity_before,
                    puck_angular_velocity_before,
                );
            let blue_net_collision = blue_net_collision
                | do_puck_net_forces(
                    puck,
                    &rink.blue_net,
                    puck_linear_velocity_before,
                    puck_angular_velocity_before,
                );

            if red_net_collision {
                events.push(PhysicsEvent::PuckTouchedNet {
                    team: Team::Red,
                    puck: *puck_index,
                })
            }
            if blue_net_collision {
                events.push(PhysicsEvent::PuckTouchedNet {
                    team: Team::Blue,
                    puck: *puck_index,
                })
            }
        }
    }
}

fn update_stick(
    player: &mut SkaterObject,
    input: &PlayerInput,
    linear_velocity_before: Vec3,
    angular_velocity_before: Vec3,
    rink: &Rink,
) {
    let stick_input = Vec2::new(
        replace_nan(input.stick[0], 0.0).clamp(-FRAC_PI_2, FRAC_PI_2),
        replace_nan(input.stick[1], 0.0).clamp(-5.0 * PI / 16.0, FRAC_PI_8),
    );

    let placement_diff = stick_input - player.stick_placement;
    let placement_change = 0.0625 * placement_diff - 0.5 * player.stick_placement_delta;
    let placement_change = placement_change.clamp_length_max(0.008_888_889);

    player.stick_placement_delta += placement_change;
    player.stick_placement += &player.stick_placement_delta;

    // Now that stick placement has been calculated,
    // we will use it to calculate the stick position and rotation

    let mul = match player.hand {
        SkaterHand::Right => 1.0,
        SkaterHand::Left => -1.0,
    };
    player.stick_rot = {
        let stick_pos_local = player.body.rot.inverse() * (player.stick_pos - player.body.pos)
            - Vec3::new(-0.375 * mul, -0.5, -0.125);

        let current_azimuth = stick_pos_local[0].atan2(-stick_pos_local[2]);
        let current_inclination = -stick_pos_local[1]
            .atan2((stick_pos_local[0].powi(2) + stick_pos_local[2].powi(2)).sqrt());
        let stick_angle = replace_nan(input.stick_angle, 0.0).clamp(-1.0, 1.0) * FRAC_PI_4;

        player.body.rot
            * Rot3::from_euler(
                EulerRot::YXY,
                -current_azimuth,
                -current_inclination,
                -player.stick_placement[1].max(0.0) * mul * FRAC_PI_2,
            )
            * Rot3::from_axis_angle(Vec3::new(0.0, 0.6, 0.8), stick_angle)
    };

    let intended_stick_rotation = player.body.rot
        * Rot3::from_euler(
            EulerRot::YXZ,
            -player.stick_placement[0],
            -(player.stick_placement[1] + FRAC_PI_4),
            0.0,
        );

    let stick_grip_position =
        player.body.pos + (player.body.rot * Vec3::new(-0.375 * mul, 0.5, -0.125));
    let mut intended_stick_position =
        stick_grip_position + intended_stick_rotation * (1.75 * Vec3::NEG_Z);
    if intended_stick_position[1] < 0.0 {
        intended_stick_position[1] = 0.0;
    }

    let speed_at_stick_pos = speed_of_point_including_rotation(
        intended_stick_position,
        player.body.pos,
        linear_velocity_before,
        angular_velocity_before,
    );
    let stick_force = 0.125 * (intended_stick_position - player.stick_pos)
        + 0.5 * (speed_at_stick_pos - player.stick_velocity);

    player.stick_velocity += 0.996 * stick_force;
    player
        .body
        .apply_acceleration_to_object(-0.004 * stick_force, intended_stick_position);

    if let Some((overlap, normal)) =
        collision_between_sphere_and_rink(player.stick_pos, 0.09375, rink)
    {
        let mut n = overlap * 0.25 * normal - 0.5 * player.stick_velocity;
        if n.dot(normal) > 0.0 {
            limit_friction(&mut n, normal, 0.1);
            player.stick_velocity += n;
        }
    }
}

fn update_player(
    i: usize,
    player: &mut SkaterObject,
    input: &PlayerInput,
    physics_config: &PhysicsConfiguration,
    rink: &Rink,
    collisions: &mut CollisionList,
) {
    let linear_velocity_before = player.body.linear_velocity;
    let angular_velocity_before = player.body.angular_velocity;

    player.body.pos += player.body.linear_velocity;
    player.body.linear_velocity[1] -= physics_config.gravity;
    for collision_ball in player.collision_balls.iter_mut() {
        collision_ball.velocity *= 0.999;
        collision_ball.pos += &collision_ball.velocity;
        collision_ball.velocity[1] -= physics_config.gravity;
    }
    let feet_pos = player.body.pos - player.body.rot * (player.height * Vec3::Y);
    if feet_pos[1] < 0.0 {
        // If feet is below ground
        let fwbw_from_client = input.fwbw.clamp(-1.0, 1.0);
        if fwbw_from_client != 0.0 {
            let mut skate_direction = if fwbw_from_client > 0.0 {
                player.body.rot * Vec3::NEG_Z // Which direction do we want to accelerate in
            } else {
                player.body.rot * Vec3::Z
            };
            let max_acceleration = if player.body.linear_velocity.dot(skate_direction) < 0.0 {
                physics_config.player_deceleration // If we're accelerating against the current direction of movement
            // we're decelerating and can do so faster
            } else {
                physics_config.player_acceleration
            };
            skate_direction[1] = 0.0;
            skate_direction = skate_direction.normalize(); // Flatten direction vector to 2d space, ignore Y axis
            let new_acceleration =
                physics_config.max_player_speed * skate_direction - player.body.linear_velocity;
            // Calculates the step needed to change from current velocity to max velocity in the desired direction in
            // a single frame. This would be way too fast, so we limit the step to a specified max acceleration

            player.body.linear_velocity += new_acceleration.clamp_length_max(max_acceleration);
        }
        if input.jump() && !player.jumped_last_frame {
            let diff = if physics_config.limit_jump_speed {
                (0.025 - player.body.linear_velocity[1]).clamp(0.0, 0.025)
            } else {
                0.025 // 0.025 is the jump acceleration per frame when jumping
            };
            if diff != 0.0 {
                player.body.linear_velocity[1] += diff;
                for collision_ball in player.collision_balls.iter_mut() {
                    collision_ball.velocity[1] += diff;
                }
            }
        }
    }
    player.jumped_last_frame = input.jump();

    // Turn player
    let turn = input.turn.clamp(-1.0, 1.0);
    if input.shift() {
        let mut velocity_direction = player.body.rot * Vec3::X; // Axis pointing towards the side (left, right? I forgot)
        velocity_direction[1] = 0.0;
        velocity_direction = velocity_direction.normalize(); // Remove Y axis, so a vector in the X-Z plane

        let velocity_adjustment =
            (physics_config.max_player_shift_speed * turn * velocity_direction)
                - player.body.linear_velocity;
        // Change required to change velocity to max speed in the desired direction in one frame
        // Still way too fast, so we limit the maximum allowed change in a single frame
        player.body.linear_velocity +=
            velocity_adjustment.clamp_length_max(physics_config.player_shift_acceleration);
        let turn_change =
            (turn * physics_config.player_shift_turning) * (player.body.rot * Vec3::Y);
        player.body.angular_velocity += turn_change;
    } else {
        // Regular turn, so let's just turn the player around its Y axis
        let turn_change = (-turn * physics_config.player_turning) * (player.body.rot * Vec3::Y);
        player.body.angular_velocity += turn_change;
    }

    if player.body.angular_velocity.length() > 1.0 / 65536.0 {
        player.body.rot = Rot3::from_scaled_axis(player.body.angular_velocity) * player.body.rot;
    }
    adjust_head_body_rot(
        &mut player.head_rot,
        input.head_rot.clamp(-7.0 * FRAC_PI_8, 7.0 * FRAC_PI_8),
    );
    adjust_head_body_rot(
        &mut player.body_rot,
        input.body_rot.clamp(-FRAC_PI_2, FRAC_PI_2),
    );
    for (collision_ball_index, collision_ball) in player.collision_balls.iter_mut().enumerate() {
        let mut new_rot = player.body.rot;
        if collision_ball_index == 1 || collision_ball_index == 2 || collision_ball_index == 5 {
            new_rot *=
                Rot3::from_euler(EulerRot::YXZ, -player.head_rot * 0.5, -player.body_rot, 0.0);
        }
        let intended_collision_ball_pos = player.body.pos + (new_rot * collision_ball.offset);
        // With head and body rotations and offset, calculate where each ball is "supposed to be"
        let collision_pos_diff = intended_collision_ball_pos - collision_ball.pos;

        let speed = speed_of_point_including_rotation(
            intended_collision_ball_pos,
            player.body.pos,
            linear_velocity_before,
            angular_velocity_before,
        );
        let force = 0.125 * collision_pos_diff + 0.25 * (speed - collision_ball.velocity);
        collision_ball.velocity += 0.9375 * force;
        player
            .body
            .apply_acceleration_to_object((0.9375 - 1.0) * force, intended_collision_ball_pos);
    }

    for (ib, collision_ball) in player.collision_balls.iter().enumerate() {
        let collision = collision_between_collision_ball_and_rink(collision_ball, rink);
        if let Some((overlap, normal)) = collision {
            collisions.push(Collision::PlayerRink((i, ib), overlap, normal));
        }
    }
    let linear_velocity_before = player.body.linear_velocity;
    let angular_velocity_before = player.body.angular_velocity;

    if input.crouch() {
        player.height = (player.height - 0.015625).max(0.25)
    } else {
        player.height = (player.height + 0.125).min(0.75);
    }

    let feet_pos = player.body.pos - player.body.rot * (player.height * Vec3::Y);
    let mut touches_ice = false;
    if feet_pos[1] < 0.0 {
        // Makes players bounce up if their feet get below the ice
        let unit_y = Vec3::Y;

        let ice_spring_force =
            0.25 * ((-feet_pos[1] * 0.125 * 0.125) * unit_y - player.body.linear_velocity);
        if ice_spring_force.dot(unit_y) > 0.0 {
            let (axis, rejection_limit) = if input.shift() {
                (Vec3::X, 0.4) // Shift means you move sideways
            } else {
                (Vec3::Z, 1.2) // If not shift, then usual forwards/backwards movement
            };
            let direction = player.body.rot * axis;
            let direction = Vec3::new(direction.x, 0.0, direction.z).normalize();

            let mut acceleration = ice_spring_force.reject_from(direction);
            // We get the rejection here, so the acceleration will be a vector perpendicular to and
            // pointing away from direction

            limit_friction(&mut acceleration, unit_y, rejection_limit);
            player.body.linear_velocity += acceleration;
            touches_ice = true;
        }
    }
    if player.body.pos[1] < 0.5 && player.body.linear_velocity.length() < 0.025 {
        player.body.linear_velocity[1] += 0.00055555555; // Extra speed boost upwards if body is low (fallen?) and the speed is slow
        touches_ice = true;
    }
    if touches_ice {
        // This is where the leaning happens
        player.body.angular_velocity *= 0.975;
        let mut intended_up = Vec3::Y;

        if !input.shift() {
            // If we're turning and not shift-turning, we want to lean while turning depending on speed
            let axis = player.body.rot * Vec3::Z;
            let fraction_of_max_speed =
                player.body.linear_velocity.dot(axis) / physics_config.max_player_speed;
            intended_up = intended_up.rotate_axis(axis, 0.225 * turn * fraction_of_max_speed);
        }

        let rotation1 = (player.body.rot * Vec3::Y).cross(intended_up);
        if let Some(rotation1_direction) = rotation1.try_normalize() {
            let angular_change = 0.008333333 * rotation1
                - 0.25
                    * player
                        .body
                        .angular_velocity
                        .project_onto(rotation1_direction);
            let angular_change = angular_change.clamp_length_max(0.000_347_222_23);
            player.body.angular_velocity += angular_change;
        }
    }
    update_stick(
        player,
        input,
        linear_velocity_before,
        angular_velocity_before,
        rink,
    );
}

fn apply_collisions(
    players: &mut [(PlayerId, &mut SkaterObject, &PlayerInput)],
    collisions: &[Collision],
) {
    for _ in 0..16 {
        let original_ball_velocities =
            ArrayVec::<_, 32>::from_iter(players.iter().map(|(_, skater, _)| {
                SmallVec::<[_; 8]>::from_iter(skater.collision_balls.iter().map(|x| x.velocity))
            }));

        for collision_event in collisions.iter() {
            match *collision_event {
                Collision::PlayerRink((i, ib), overlap, normal) => {
                    let original_velocity = original_ball_velocities[i][ib];
                    let mut new = overlap * 0.03125 * normal - 0.25 * original_velocity;
                    if new.dot(normal) > 0.0 {
                        limit_friction(&mut new, normal, 0.01);
                        let (_, skater, _) = &mut players[i];
                        let ball = &mut skater.collision_balls[ib];
                        ball.velocity += new;
                    }
                }
                Collision::PlayerPlayer((i, ib), (j, jb), overlap, normal) => {
                    let original_velocity1 = original_ball_velocities[i][ib];
                    let original_velocity2 = original_ball_velocities[j][jb];

                    let mut new =
                        overlap * 0.125 * normal + 0.25 * (original_velocity2 - original_velocity1);
                    if new.dot(normal) > 0.0 {
                        limit_friction(&mut new, normal, 0.01);
                        let (_, skater1, _) = &players[i];
                        let (_, skater2, _) = &players[j];
                        let mass1 = skater1.collision_balls[ib].mass;
                        let mass2 = skater2.collision_balls[jb].mass;
                        let mass_sum = mass1 + mass2;

                        let (_, skater1, _) = &mut players[i];
                        skater1.collision_balls[ib].velocity += (mass2 / mass_sum) * new;

                        let (_, skater2, _) = &mut players[j];
                        skater2.collision_balls[jb].velocity -= (mass1 / mass_sum) * new;
                    }
                }
            }
        }
    }
}

fn puck_detection(
    puck: &mut Puck,
    puck_index: PuckId,
    old_puck_pos: Vec3,
    rink: &Rink,
    events: &mut PhysicsEventList,
) {
    let puck_pos = puck.body.pos;

    fn check_lines(
        puck_index: PuckId,
        puck_pos: Vec3,
        old_puck_pos: Vec3,
        puck_radius: f32,
        team: Team,
        rink: &Rink,
        events: &mut PhysicsEventList,
    ) {
        let (own_side, other_side, defensive_line, offensive_line) = match team {
            Team::Red => (
                RedSide,
                BlueSide,
                &rink.red_zone_blue_line,
                &rink.blue_zone_blue_line,
            ),
            Team::Blue => (
                BlueSide,
                RedSide,
                &rink.blue_zone_blue_line,
                &rink.red_zone_blue_line,
            ),
        };
        let old_position = defensive_line.side_of_line(old_puck_pos, puck_radius);
        let position = defensive_line.side_of_line(puck_pos, puck_radius);

        if old_position == own_side && position != own_side {
            let event = PhysicsEvent::PuckReachedDefensiveLine {
                team,
                puck: puck_index,
            };
            events.push(event);
        }
        if position == other_side && old_position != other_side {
            let event = PhysicsEvent::PuckPassedDefensiveLine {
                team,
                puck: puck_index,
            };
            events.push(event);
        }
        let old_position = rink.center_line.side_of_line(old_puck_pos, puck_radius);
        let position = rink.center_line.side_of_line(puck_pos, puck_radius);

        if old_position == own_side && position != own_side {
            let event = PhysicsEvent::PuckReachedCenterLine {
                team,
                puck: puck_index,
            };
            events.push(event);
        }
        if position == other_side && old_position != other_side {
            let event = PhysicsEvent::PuckPassedCenterLine {
                team,
                puck: puck_index,
            };
            events.push(event);
        }

        let old_position = offensive_line.side_of_line(old_puck_pos, puck_radius);
        let position = offensive_line.side_of_line(puck_pos, puck_radius);

        if old_position == own_side && position != own_side {
            let event = PhysicsEvent::PuckReachedOffensiveZone {
                team,
                puck: puck_index,
            };
            events.push(event);
        }
        if position == other_side && old_position != other_side {
            let event = PhysicsEvent::PuckEnteredOffensiveZone {
                team,
                puck: puck_index,
            };
            events.push(event);
        }
    }

    fn check_net(
        puck_index: PuckId,
        puck_pos: Vec3,
        old_puck_pos: Vec3,
        net: &RinkNet,
        team: Team,
        events: &mut PhysicsEventList,
    ) {
        if (net.left_post - puck_pos).dot(net.normal) >= 0.0
            && (net.left_post - old_puck_pos).dot(net.normal) < 0.0
        {
            if (net.left_post - puck_pos).dot(net.left_post_inside) < 0.0
                && (net.right_post - puck_pos).dot(net.right_post_inside) < 0.0
                && puck_pos.y < 1.0
            {
                let event = PhysicsEvent::PuckEnteredNet {
                    team,
                    puck: puck_index,
                };
                events.push(event);
            } else {
                let event = PhysicsEvent::PuckPassedGoalLine {
                    team,
                    puck: puck_index,
                };
                events.push(event);
            }
        }
    }

    check_lines(
        puck_index,
        puck_pos,
        old_puck_pos,
        puck.radius,
        Team::Red,
        rink,
        events,
    );
    check_lines(
        puck_index,
        puck_pos,
        old_puck_pos,
        puck.radius,
        Team::Blue,
        rink,
        events,
    );
    check_net(
        puck_index,
        puck_pos,
        old_puck_pos,
        &rink.red_net,
        Team::Red,
        events,
    );
    check_net(
        puck_index,
        puck_pos,
        old_puck_pos,
        &rink.blue_net,
        Team::Blue,
        events,
    );
}

fn do_puck_net_forces(
    puck: &mut Puck,
    net: &RinkNet,
    puck_linear_velocity: Vec3,
    puck_angular_velocity: Vec3,
) -> bool {
    let mut res = false;
    if let Some((overlap_pos, overlap, normal)) =
        collision_between_sphere_and_net(puck.body.pos, puck.radius, net)
    {
        res = true;
        let vertex_velocity = speed_of_point_including_rotation(
            overlap_pos,
            puck.body.pos,
            puck_linear_velocity,
            puck_angular_velocity,
        );
        let mut puck_force = 0.5 * overlap * normal - 0.5 * vertex_velocity;

        if normal.dot(puck_force) > 0.0 {
            limit_friction(&mut puck_force, normal, 0.5);
            puck.body
                .apply_acceleration_to_object(puck_force, overlap_pos);
            puck.body.linear_velocity *= 0.9875;
            puck.body.angular_velocity *= 0.95;
        }
    }
    res
}

fn do_puck_post_forces(
    puck: &mut Puck,
    net: &RinkNet,
    puck_linear_velocity: Vec3,
    puck_angular_velocity: Vec3,
) -> bool {
    let mut res = false;
    for post in net.posts.iter() {
        let collision = collision_between_sphere_and_post(puck.body.pos, puck.radius, post);
        if let Some((overlap, normal)) = collision {
            res = true;
            let p = puck.body.pos - puck.radius * normal;
            let vertex_velocity = speed_of_point_including_rotation(
                p,
                puck.body.pos,
                puck_linear_velocity,
                puck_angular_velocity,
            );
            let mut puck_force = overlap * 0.125 * normal - 0.25 * vertex_velocity;

            if normal.dot(puck_force) > 0.0 {
                limit_friction(&mut puck_force, normal, 0.2);
                puck.body.apply_acceleration_to_object(puck_force, p);
            }
        }
    }
    res
}

fn do_puck_stick_forces(
    puck: &mut Puck,
    player: &mut SkaterObject,
    puck_vertices: &[Vec3],
    puck_linear_velocity: Vec3,
    puck_angular_velocity: Vec3,
    stick_velocity: Vec3,
) -> bool {
    let mut res = false;
    for puck_vertex in puck_vertices.iter().copied() {
        let col = collision_between_puck_vertex_and_stick(
            puck.body.pos,
            puck_vertex,
            player.stick_pos,
            player.stick_rot,
        );
        if let Some((dot, normal)) = col {
            res = true;
            let puck_vertex_speed = speed_of_point_including_rotation(
                puck_vertex,
                puck.body.pos,
                puck_linear_velocity,
                puck_angular_velocity,
            );

            let mut puck_force =
                dot * 0.125 * 0.5 * normal + 0.125 * (stick_velocity - puck_vertex_speed);
            if puck_force.dot(normal) > 0.0 {
                limit_friction(&mut puck_force, normal, 0.5);
                player.stick_velocity -= 0.25 * puck_force;
                puck_force *= 0.75;
                puck.body
                    .apply_acceleration_to_object(puck_force, puck_vertex);
            }
        }
    }
    res
}

fn do_puck_rink_forces(
    puck: &mut Puck,
    puck_vertices: &[Vec3],
    rink: &Rink,
    puck_linear_velocity: Vec3,
    puck_angular_velocity: Vec3,
    friction: f32,
) {
    for vertex in puck_vertices.iter().copied() {
        let c = collision_between_vertex_and_rink(vertex, rink);
        if let Some((overlap, normal)) = c {
            let vertex_velocity = speed_of_point_including_rotation(
                vertex,
                puck.body.pos,
                puck_linear_velocity,
                puck_angular_velocity,
            );
            let mut puck_force = 0.125 * 0.125 * (overlap * 0.5 * normal - vertex_velocity);

            if normal.dot(puck_force) > 0.0 {
                limit_friction(&mut puck_force, normal, friction);
                puck.body.apply_acceleration_to_object(puck_force, vertex);
            }
        }
    }
}

fn inside_surface(pos: Vec3, surface: &(Vec3, Vec3, Vec3, Vec3), normal: Vec3) -> bool {
    let (p1, p2, p3, p4) = surface;
    (pos - p1).cross(p2 - p1).dot(normal) >= 0.0
        && (pos - p2).cross(p3 - p2).dot(normal) >= 0.0
        && (pos - p3).cross(p4 - p3).dot(normal) >= 0.0
        && (pos - p4).cross(p1 - p4).dot(normal) >= 0.0
}

fn collision_between_sphere_and_net(
    pos: Vec3,
    radius: f32,
    net: &RinkNet,
) -> Option<(Vec3, f32, Vec3)> {
    let mut max_overlap = 0.0;
    let mut res: Option<(Vec3, f32, Vec3)> = None;

    for surface in net.surfaces.iter() {
        let normal = (surface.3 - surface.0)
            .cross(surface.1 - surface.0)
            .normalize();

        let diff = surface.0 - pos;
        let dot = diff.dot(normal);
        let overlap = dot + radius;
        let overlap2 = -dot + radius;

        if overlap > 0.0 && overlap < radius {
            let overlap_pos = pos + (radius - overlap) * normal;
            if inside_surface(overlap_pos, surface, normal) && overlap > max_overlap {
                max_overlap = overlap;
                res = Some((overlap_pos, overlap, normal));
            }
        } else if overlap2 > 0.0 && overlap2 < radius {
            let overlap_pos = pos + (radius - overlap) * normal;
            if inside_surface(overlap_pos, surface, normal) && overlap2 > max_overlap {
                max_overlap = overlap2;
                res = Some((overlap_pos, overlap2, -normal));
            }
        }
    }

    res
}

fn collision_between_sphere_and_post(
    pos: Vec3,
    radius: f32,
    post: &(Vec3, Vec3, f32),
) -> Option<(f32, Vec3)> {
    let (p1, p2, post_radius) = post;
    let a = post_radius + radius;
    let direction_vector = p2 - p1;

    let diff = pos - p1;
    let t0 = diff.dot(direction_vector) / direction_vector.length_squared();
    let dot = t0.clamp(0.0, 1.0);

    let projection = dot * direction_vector;
    let rejection = diff - projection;
    let rejection_norm = rejection.length();
    let overlap = a - rejection_norm;
    if overlap > 0.0 {
        Some((overlap, rejection.normalize()))
    } else {
        None
    }
}

fn collision_between_puck_vertex_and_stick(
    puck_pos: Vec3,
    puck_vertex: Vec3,
    stick_pos: Vec3,
    stick_rot: Rot3,
) -> Option<(f32, Vec3)> {
    let half_size = Vec3::new(0.03125, 0.125, 0.25); // stick_size * 0.5

    let rot_inv = stick_rot.inverse();

    // Transform both points into stick local space
    let local_origin = rot_inv * (puck_pos - stick_pos);
    let local_vertex = rot_inv * (puck_vertex - stick_pos);

    let diff = local_vertex - local_origin;

    // For each face of the AABB, test if the segment origin->vertex crosses it
    // and find the earliest crossing that's inside the box
    let mut min_intersection = 1f32;
    let mut res = None;

    for axis in 0..3 {
        for sign in [1f32, -1f32] {
            let face_d = sign * half_size[axis];
            let origin_side = local_origin[axis] - face_d;
            let vertex_side = local_vertex[axis] - face_d;

            // vertex must be on or past the face, origin must be inside or on other side
            if vertex_side * sign <= 0.0 && origin_side * sign >= 0.0 {
                let diff_axis = diff[axis];
                if diff_axis != 0.0 {
                    let intersection = -origin_side / diff_axis;
                    if intersection < min_intersection {
                        let intersection_pos = local_origin + intersection * diff;
                        // Check inside the face bounds (the other two axes)
                        let a = (axis + 1) % 3;
                        let b = (axis + 2) % 3;
                        if intersection_pos[a].abs() <= half_size[a]
                            && intersection_pos[b].abs() <= half_size[b]
                        {
                            let overlap = (intersection_pos[axis] - local_vertex[axis]) * sign;
                            // Normal in local space is just the axis unit vector
                            let local_normal = sign
                                * Vec3::from({
                                    let mut a = [0f32; 3];
                                    a[axis] = 1.0;
                                    a
                                });
                            // Transform normal back to world space
                            let world_normal = stick_rot * local_normal;
                            min_intersection = intersection;
                            res = Some((overlap, world_normal));
                        }
                    }
                }
            }
        }
    }
    res
}

fn collision_between_sphere_and_rink(pos: Vec3, radius: f32, rink: &Rink) -> Option<(f32, Vec3)> {
    let mut max_overlap = 0f32;
    let mut coll_normal = None;
    for (p, normal) in rink.planes.iter() {
        let overlap = (p - pos).dot(*normal) + radius;
        if overlap > max_overlap {
            max_overlap = overlap;
            coll_normal = Some(*normal);
        }
    }
    for (p, dir, corner_radius) in rink.corners.iter() {
        let mut p2 = p - pos;
        p2[1] = 0.0;
        if p2[0] * dir[0] < 0.0 && p2[2] * dir[2] < 0.0 {
            let overlap = p2.length() + radius - corner_radius;
            if overlap > max_overlap {
                max_overlap = overlap;
                coll_normal = Some(p2.normalize());
            }
        }
    }
    coll_normal.map(|n| (max_overlap, n))
}

fn collision_between_collision_ball_and_rink(
    ball: &SkaterCollisionBall,
    rink: &Rink,
) -> Option<(f32, Vec3)> {
    collision_between_sphere_and_rink(ball.pos, ball.radius, rink)
}

fn collision_between_vertex_and_rink(vertex: Vec3, rink: &Rink) -> Option<(f32, Vec3)> {
    collision_between_sphere_and_rink(vertex, 0.0, rink)
}

fn speed_of_point_including_rotation(
    p: Vec3,
    pos: Vec3,
    linear_velocity: Vec3,
    angular_velocity: Vec3,
) -> Vec3 {
    linear_velocity + angular_velocity.cross(p - pos)
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

pub fn limit_friction(v: &mut Vec3, normal: Vec3, d: f32) {
    let projection = v.project_onto_normalized(normal);
    let rejection = v.reject_from_normalized(normal);
    *v = projection;

    if rejection.length() > 1.0 / 65536.0 {
        *v += rejection.clamp_length_max(projection.length() * d);
    }
}

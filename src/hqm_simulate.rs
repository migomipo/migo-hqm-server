use crate::{HQMServer, HQMGameObject, HQMSkater, HQMBody, HQMPuck, HQMRink, HQMSkaterCollisionBall};
use nalgebra::{Vector3, Matrix3, U3, U1, Matrix, Vector2, Point3};
use std::ops::{Sub, AddAssign};
use nalgebra::base::storage::{Storage, StorageMut};

const GRAVITY: f32 = 0.000680;
impl HQMServer {


    pub(crate) fn simulate_step (&mut self) {
        let mut players = Vec::new();
        let mut pucks = Vec::new();
        for o in self.game.objects.iter_mut() {
            match o {
                HQMGameObject::Player(player) => players.push(player),
                HQMGameObject::Puck(puck) => pucks.push(puck),
                _ => {}
            }
        }

        for player in players.iter_mut() {
            update_player(player);
            let pos_delta_copy = player.body.linear_velocity.clone_owned();
            let rot_axis_copy = player.body.angular_velocity.clone_owned();
            update_player2(player, & self.game.rink);
            update_stick(player, & pos_delta_copy, & rot_axis_copy);
            player.old_input = player.input.clone();
        }
        for puck in pucks.iter_mut() {
            puck.body.linear_velocity[1] -= GRAVITY;

        }

        for i in 0..10 {

            for player in players.iter_mut() {
                player.stick_pos += player.stick_velocity.scale(0.1);
            }
            for puck in pucks.iter_mut() {
                puck.body.pos += puck.body.linear_velocity.scale(0.1);

                let old_pos_delta = puck.body.linear_velocity.clone_owned();
                let old_rot_axis = puck.body.angular_velocity.clone_owned();
                let puck_vertices = get_puck_vertices(&puck.body.pos, &puck.body.rot, puck.height, puck.radius);
                if i == 0 {
                    collisions_between_puck_and_rink(puck, & puck_vertices, & self.game.rink, &old_pos_delta, &old_rot_axis);
                }
                for player in players.iter_mut() {
                    let old_stick_pos_delta = player.stick_velocity.clone_owned();
                    if (&puck.body.pos - &player.stick_pos).norm() < 1.0 {
                        collisions_between_puck_and_stick(puck, player, & puck_vertices, &old_pos_delta, &old_rot_axis, &old_stick_pos_delta);
                    }
                }
            }

        }
        for puck in pucks.iter_mut() {
            if puck.body.linear_velocity.norm () > 0.000015258789 {

                let scale = puck.body.linear_velocity.norm ().powi(2) * 0.125 * 0.125;
                let scaled = puck.body.linear_velocity.normalize().scale(scale);
                puck.body.linear_velocity -= scaled;


            }
            if puck.body.angular_velocity.norm() > 0.000015258789 {
                rotate_matrix_around_axis(& mut puck.body.rot, &puck.body.angular_velocity.normalize(), puck.body.angular_velocity.norm())
            }
        }

    }
}

fn collision_between_vertex_and_stick2(puck_pos: &Point3<f32>, vertex: &Point3<f32>, p1: &Point3<f32>, p2: &Point3<f32>, p3: &Point3<f32>) -> Option<(f32, Vector3<f32>, f32)> {
    let normal = (p3 - p1).cross(&(p2 - p1)).normalize();
    if (p1 - vertex).dot(&normal) >= 0.0 {
        let dot2 = (p1 - puck_pos).dot(&normal);
        if dot2 <= 0.0 {
            let diff = vertex - puck_pos;
            let dot3 = diff.dot(&normal);
            if dot3 != 0.0 {
                let overlap = dot2/dot3;
                let overlap_pos = puck_pos + diff.scale(overlap);
                if (overlap_pos - p1).cross(&(p2 - p1)).dot(&normal) >= 0.0 &&
                    (overlap_pos - p2).cross(&(p3 - p2)).dot(&normal) >= 0.0 &&
                    (overlap_pos - p3).cross(&(p1 - p3)).dot(&normal) >= 0.0 {
                    let dot_res = (overlap_pos - vertex).dot(&normal);
                    return Some((overlap, normal, dot_res));
                }
            }
        }
    }
    None
}

fn collision_between_vertex_and_stick(puck_pos: &Point3<f32>, vertex: &Point3<f32>, stick_planes: &Vec<(Point3<f32>,Point3<f32>,Point3<f32>,Point3<f32>)>) -> Option<(f32, Vector3<f32>)> {
    let mut overlap = 1f32;
    let mut res = None;
    for plane in stick_planes.iter() {
        let col1 = collision_between_vertex_and_stick2(puck_pos, vertex, &plane.0, &plane.1, &plane.2);
        let col2 = collision_between_vertex_and_stick2(puck_pos, vertex, &plane.0, &plane.2, &plane.3);
        if let Some((o, normal, dot)) = col1 {
            if o < overlap {
                res = Some((dot, normal));
                overlap = o;
            }
        }
        if let Some((o, normal, dot)) = col2 {
            if o < overlap {
                res = Some((dot, normal));
                overlap = o;
            }
        }
    }
    res
}


fn collisions_between_puck_and_stick(puck: & mut HQMPuck, player: & mut HQMSkater, puck_vertices: &Vec<Point3<f32>>,
                                     old_pos_delta: & Vector3<f32>, old_rot_axis: & Vector3<f32>, old_stick_pos_delta: & Vector3<f32>) {
    let stick_planes = get_stick_planes(player);
    //let mut total_overlap = 0.0;
    //let mut total_vertex_collisions = 0;
    for puck_vertex in puck_vertices.iter() {
        let col = collision_between_vertex_and_stick(& puck.body.pos, puck_vertex, &stick_planes);
        if let Some ((dot, normal)) = col {
            //println! ("{} {:?}", dot, normal);
            //total_vertex_collisions += 1;
            //total_overlap += dot;

            let puck_vertex_speed = speed_of_point_including_rotation(&puck_vertex, & puck.body.pos, old_pos_delta, old_rot_axis);

            let mut puck_force = (normal.scale(dot * 0.5) + (old_stick_pos_delta - puck_vertex_speed)).scale(0.125);
            if puck_force.dot(&normal) > 0.0 {
                limit_rejection(& mut puck_force, &normal, 0.5);
                player.stick_velocity -= puck_force.scale(0.25);
                puck_force.scale_mut(0.75);
                apply_acceleration_to_object(& mut puck.body, &puck_force, & puck_vertex);
            }
        }
    }
    //if total_vertex_collisions > 0 {
    //    println!("{} collisions with total overlap {}", total_vertex_collisions, total_overlap);
    //    println!("================");
    //}
}

fn get_stick_planes (player: & HQMSkater) -> Vec<(Point3<f32>,Point3<f32>,Point3<f32>,Point3<f32>)> {
    let stick_size = Vector3::new(0.0625, 0.25, 0.5);
    let nnn = &player.stick_pos + &player.stick_rot * Vector3::new(-0.5, -0.5, -0.5).component_mul(&stick_size);
    let nnp = &player.stick_pos + &player.stick_rot * Vector3::new(-0.5, -0.5,  0.5).component_mul(&stick_size);
    let npn = &player.stick_pos + &player.stick_rot * Vector3::new(-0.5,  0.5, -0.5).component_mul(&stick_size);
    let npp = &player.stick_pos + &player.stick_rot * Vector3::new(-0.5,  0.5,  0.5).component_mul(&stick_size);
    let pnn = &player.stick_pos + &player.stick_rot * Vector3::new( 0.5, -0.5, -0.5).component_mul(&stick_size);
    let pnp = &player.stick_pos + &player.stick_rot * Vector3::new( 0.5, -0.5,  0.5).component_mul(&stick_size);
    let ppn = &player.stick_pos + &player.stick_rot * Vector3::new( 0.5,  0.5, -0.5).component_mul(&stick_size);
    let ppp = &player.stick_pos + &player.stick_rot * Vector3::new( 0.5,  0.5,  0.5).component_mul(&stick_size);

    let res = vec![
        (nnp.clone(), pnp.clone(), pnn.clone(), nnn.clone()),
        (npp.clone(), ppp.clone(), pnp.clone(), nnp.clone()),
        (npn.clone(), npp.clone(), nnp.clone(), nnn.clone()),
        (ppn.clone(), npn.clone(), nnn.clone(), pnn.clone()),
        (ppp.clone(), ppn.clone(), pnn.clone(), pnp.clone()),
        (npn.clone(), ppn.clone(), ppp.clone(), npp.clone())
    ];
    res

}

fn collisions_between_puck_and_rink(puck: & mut HQMPuck, puck_vertices: &Vec<Point3<f32>>, rink: & HQMRink, old_pos_delta: & Vector3<f32>, old_rot_axis: & Vector3<f32>) {
    for vertex in puck_vertices.iter() {
        let c = collision_between_vertex_and_rink(vertex, rink);
        if let Some((projection, normal)) = c {
            let vertex_velocity = speed_of_point_including_rotation(&vertex, &puck.body.pos, old_pos_delta, old_rot_axis);
            let mut puck_force = (normal.scale(projection * 0.5) - vertex_velocity).scale(0.125 * 0.125);

            if normal.dot (&puck_force) > 0.0 {
                limit_rejection(& mut puck_force, & normal, 0.05);
                apply_acceleration_to_object(& mut puck.body, &puck_force, &vertex);
            }
        }
    }
}

fn collision_between_collision_ball_and_rink(ball: &HQMSkaterCollisionBall, rink: & HQMRink) -> Option<(f32, Vector3<f32>)> {
    let mut max_proj = 0f32;
    let mut coll_normal  = None;
    for (p, normal) in rink.planes.iter() {

        let proj = (p - &ball.pos).dot (normal) + ball.radius;
        if proj > max_proj {
            max_proj = proj;
            coll_normal = Some(normal.clone_owned());
        }
    }
    for (p, dir, radius) in rink.corners.iter() {
        let mut p2 = p - &ball.pos;
        p2[1] = 0.0;
        if p2[0]*dir[0] < 0.0 && p2[2]*dir[2] < 0.0 {
            let diff = p2.norm() + ball.radius - radius;
            if diff > max_proj {
                max_proj = diff;
                let p2n = p2.normalize();
                coll_normal = Some(p2n);
            }

        }
    }
    match coll_normal {
        Some(n) => Some((max_proj, n)),
        None => None
    }
}

fn collision_between_vertex_and_rink(vertex: &Point3<f32>, rink: & HQMRink) -> Option<(f32, Vector3<f32>)> {
    let mut max_proj = 0f32;
    let mut coll_normal  = None;
    for (p, normal) in rink.planes.iter() {
        let proj = (p - vertex).dot (normal);
        if proj > max_proj {
            max_proj = proj;
            coll_normal = Some(normal.clone_owned());
        }
    }
    for (p, dir, radius) in rink.corners.iter() {
        let mut p2 = p - vertex;
        p2[1] = 0.0;
        if p2[0]*dir[0] < 0.0 && p2[2]*dir[2] < 0.0 {
            let diff = p2.norm() - radius;
            if diff > max_proj {
                max_proj = diff;
                let p2n = p2.normalize();
                coll_normal = Some(p2n);
            }

        }
    }

    match coll_normal {
        Some(n) => Some((max_proj, n)),
        None => None
    }
}

fn get_puck_vertices (pos: & Point3<f32>, rot: & Matrix3<f32>, height: f32, radius: f32) -> Vec<Point3<f32>> {
    let mut res = Vec::with_capacity(48);
    for i in 0..16 {

        let (sin, cos) = ((i as f32)*std::f32::consts::PI/8.0).sin_cos();
        for j in -1..=1 {
            let point = Vector3::new(cos * radius, (j as f32)*height, sin * radius);
            let point2 = rot * point;
            res.push(pos + point2);
        }
    }
    res
}

fn update_player(player: & mut HQMSkater) {
    let old_pos_delta = player.body.linear_velocity.clone_owned();
    let old_rot_axis = player.body.angular_velocity.clone_owned();

    player.body.pos += &player.body.linear_velocity;
    player.body.linear_velocity[1] -= GRAVITY;
    for collision_ball in player.collision_balls.iter_mut() {
        collision_ball.velocity *= 0.999;
        collision_ball.pos += &collision_ball.velocity;
        collision_ball.velocity[1] -= GRAVITY;
    }
    let feet_pos = &player.body.pos - player.body.rot.column(1).scale(player.height);
    if feet_pos[1] < 0.0 {
        let fwbw_from_client = player.input.fwbw;

        if fwbw_from_client > 0.0 {
            let col2 = player.body.rot.column(2);
            let mut skate_direction = -col2.clone_owned();
            skate_direction[1] = 0.0;
            skate_direction.normalize_mut();
            skate_direction.scale_mut(0.05);
            skate_direction -= &player.body.linear_velocity;
            let max_acceleration = if player.body.linear_velocity.dot(&col2) > 0.0 {
                0.000555555f32
            } else {
                0.000208333f32
            };
            player.body.linear_velocity += limit_vector_length(&skate_direction, max_acceleration);
        } else if fwbw_from_client < 0.0 {
            let col2 = player.body.rot.column(2);
            let mut skate_direction = col2.clone_owned();
            skate_direction[1] = 0.0;
            skate_direction.normalize_mut();
            skate_direction.scale_mut(0.05);
            skate_direction -= &player.body.linear_velocity;
            let vector_length_limit = if player.body.linear_velocity.dot(&col2) < 0.0 {
                0.000555555f32
            } else {
                0.000208333f32
            };
            player.body.linear_velocity += limit_vector_length(&skate_direction, vector_length_limit);
        }
        if player.input.jump() && !player.old_input.jump() {
            player.body.linear_velocity[1] += 0.025;
            for collision_ball in player.collision_balls.iter_mut() {
                collision_ball.velocity[1] += 0.025;
            }
        }
    }

    let turn = clamp(player.input.turn, -1.0, 1.0);
    if turn != 0.0 {
        let mut column = player.body.rot.column(1).clone_owned();
        column.scale_mut(turn * 6.0 / 14400.0);
        player.body.angular_velocity += column;
    }
    // Turn player
    if player.body.angular_velocity.norm() > 0.00001 {
        rotate_matrix_around_axis(& mut player.body.rot, &player.body.angular_velocity.normalize(), player.body.angular_velocity.norm());
    }
    adjust_head_body_rot(& mut player.head_rot, player.input.head_rot);
    adjust_head_body_rot(& mut player.body_rot, player.input.body_rot);
    for (collision_ball_index, collision_ball) in player.collision_balls.iter_mut().enumerate() {
        let mut new_rot = player.body.rot.clone_owned();
        if collision_ball_index == 1 || collision_ball_index == 2 || collision_ball_index == 5 {
            let rot_axis = new_rot.column(1).clone_owned();
            rotate_matrix_around_axis(& mut new_rot, & rot_axis, player.head_rot * 0.5);
            let rot_axis = new_rot.column(0).clone_owned();
            rotate_matrix_around_axis(& mut new_rot, & rot_axis, player.body_rot);
        }
        let intended_collision_ball_pos = &player.body.pos + (new_rot * &collision_ball.offset);
        let collision_pos_diff = intended_collision_ball_pos - &collision_ball.pos;
        //println!("{:?}", collision_pos_diff);
        let speed = speed_of_point_including_rotation(& intended_collision_ball_pos, & player.body.pos, & old_pos_delta, & old_rot_axis);
        let force = collision_pos_diff.scale(0.125) + (speed - &collision_ball.velocity).scale(0.25);
        collision_ball.velocity += force.scale(0.9375);
        apply_acceleration_to_object(& mut player.body, &force.scale(0.9375 - 1.0), &intended_collision_ball_pos);
    }

}

fn update_player2 (player: & mut HQMSkater, rink: & HQMRink) {
    for collision_ball in player.collision_balls.iter() {
        let collision = collision_between_collision_ball_and_rink(collision_ball, rink);
        if let Some((overlap, normal)) = collision {

        }
    }
    let turn = clamp(player.input.turn, -1.0, 1.0);
    if player.input.crouch() {
        player.height = (player.height - 0.015625).max(0.25)
    } else {
        player.height = (player.height + 0.125).min (0.75);
    }
    let feet_pos = &player.body.pos - player.body.rot.column(1).scale(player.height);
    let mut touches_ice = false;
    if feet_pos[1] < 0.0 {
        // Makes players bounce up if their feet get below the ice
        let temp1 = -feet_pos[1] * 0.125 * 0.125 * 0.25;
        let unit_y = Vector3::y();

        let mut temp2 = unit_y.scale(temp1) - player.body.linear_velocity.scale(0.25);
        if temp2.dot(&unit_y) > 0.0 {
            let mut temp_v2 = player.body.rot.column(2).clone_owned();
            temp_v2[1] = 0.0;
            normal_or_zero_mut(& mut temp_v2);

            temp2 -= temp_v2.scale(temp2.dot(&temp_v2));
            limit_rejection(& mut temp2, & unit_y, 1.2);
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
        let temp = -player.body.linear_velocity.dot(&player.body.rot.column(2)) / 0.05;
        rotate_vector_around_axis(& mut unit, &player.body.rot.column(2), 0.225 * turn * temp);

        let mut temp2 = unit.cross(&player.body.rot.column(1));

        let temp2n = normal_or_zero(&temp2);
        temp2.scale_mut(0.008333333);

        let temp3 = -0.25 * temp2n.dot (&player.body.angular_velocity);
        temp2 += temp2n.scale(temp3);
        temp2 = limit_vector_length(&temp2, 0.000347);
        player.body.angular_velocity += temp2;
    }

}

fn update_stick(player: & mut HQMSkater, old_pos_delta: & Vector3<f32>, old_rot_axis: & Vector3<f32>) {
    let placement_diff = &player.input.stick - &player.stick_placement;
    let mut placement_temp = placement_diff.scale(0.0625) - player.stick_placement_delta.scale(0.5);
    limit_vector_length_mut2(& mut placement_temp, 0.0088888891);
    player.stick_placement_delta += placement_temp;
    player.stick_placement += &player.stick_placement_delta;

    // Now that stick placement has been calculated,
    // we will use it to calculate the stick position and rotation

    let pivot1_pos = &player.body.pos + (&player.body.rot * Vector3::new(-0.375, -0.5, -0.125));
    let pivot2_pos = &player.body.pos + (&player.body.rot * Vector3::new(-0.375, 0.5, -0.125));

    let stick_pos_converted = player.body.rot.transpose() * (&player.stick_pos - pivot1_pos);

    let current_azimuth = stick_pos_converted[0].atan2(-stick_pos_converted[2]);
    let current_inclination = -stick_pos_converted[1].atan2((stick_pos_converted[0].powi(2) + stick_pos_converted[2].powi(2)).sqrt());

    let mut stick_rotation1 = player.body.rot.clone_owned();
    rotate_matrix_spherical(& mut stick_rotation1, current_azimuth, current_inclination);
    player.stick_rot = stick_rotation1;

    if player.stick_placement[1] > 0.0 {
        let col1 = player.stick_rot.column(1).clone_owned();
        rotate_matrix_around_axis(& mut player.stick_rot, & col1, player.stick_placement[1] * 0.5 * std::f32::consts::PI)
    }

    // Rotate around the stick axis
    let handle = (player.stick_rot.column(2).clone_owned() + player.stick_rot.column(1).scale(0.75)).normalize();
    rotate_matrix_around_axis(& mut player.stick_rot, & handle, -player.input.stick_angle * 0.25 * std::f32::consts::PI);

    let mut stick_rotation2 = player.body.rot.clone_owned();
    rotate_matrix_spherical(& mut stick_rotation2, player.stick_placement[0], player.stick_placement[1]);

    let temp = stick_rotation2.column(0).clone_owned();
    rotate_matrix_around_axis(& mut stick_rotation2, & temp, 0.25 * std::f32::consts::PI);

    let mut temp_pos2 = pivot2_pos + stick_rotation2.column(2).scale(-1.75);
    if temp_pos2[1] < 0.0 {
        temp_pos2[1] = 0.0;
    }

    let momentum = speed_of_point_including_rotation(& temp_pos2, & player.body.pos, old_pos_delta, old_rot_axis);
    let stick_pos_movement = 0.125 * (temp_pos2 - &player.stick_pos) - player.stick_velocity.scale(0.5) + momentum.scale(0.5);

    player.stick_velocity += stick_pos_movement.scale(0.996);
    apply_acceleration_to_object(& mut player.body, & stick_pos_movement.scale(-0.004), & temp_pos2)

}

fn apply_acceleration_to_object(body: & mut HQMBody, change: & Vector3<f32>, point: & Point3<f32>) {
    let diff1 = point - &body.pos;
    body.linear_velocity += change;
    let cross = change.cross(& diff1);
    body.angular_velocity += &body.rot * (body.rot.transpose() * cross).component_mul(& body.rot_mul);
}

fn speed_of_point_including_rotation(p: & Point3<f32>, pos: & Point3<f32>, old_pos_delta: & Vector3<f32>, old_rot_axis: & Vector3<f32>) -> Vector3<f32> {
    old_pos_delta + (p - pos).cross(old_rot_axis)
}

fn rotate_matrix_spherical(matrix: & mut Matrix3<f32>, azimuth: f32, inclination: f32) {
    let col1 = matrix.column(1).clone_owned();
    rotate_matrix_around_axis(matrix, &col1, azimuth);
    let col0 = matrix.column(0).clone_owned();
    rotate_matrix_around_axis(matrix, &col0, inclination);
}

fn adjust_head_body_rot (rot: & mut f32, input_rot: f32)     {
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

fn clamp (v: f32, min: f32, max: f32) -> f32 {
    if v < min {
        min
    } else if v > max {
        max
    } else {
        v
    }
}

fn limit_vector_length (v: &Vector3<f32>, max_len: f32) -> Vector3<f32> {
    let norm = v.norm();
    let mut res = v.clone_owned();
    if norm > max_len {
        res.scale_mut(max_len / norm);
    }
    res
}

fn limit_vector_length_mut (v: & mut Vector3<f32>, max_len: f32) {
    let norm = v.norm();
    if norm > max_len {
        v.scale_mut(max_len / norm);
    }
}

fn limit_vector_length_mut2 (v: & mut Vector2<f32>, max_len: f32) {
    let norm = v.norm();
    if norm > max_len {
        v.scale_mut(max_len / norm);
    }
}

fn limit_rejection(v: & mut Vector3<f32>, normal: &Vector3<f32>, d: f32) {
    let projection_length = v.dot(&normal);
    let projection = normal.scale(projection_length);
    let rejection = v.sub(&projection);
    let rejection_length = rejection.norm();

    if rejection_length > 0.000015258789 {
        let rejection_norm = rejection.normalize();

        let rejection_length2 = rejection_length.min(projection.norm() * d);
        v.copy_from(&projection);
        v.add_assign(rejection_norm.scale(rejection_length2));
    }
}

fn rotate_vector_around_axis<S: StorageMut<f32, U3, U1>, T: Storage<f32, U3, U1>>(v: & mut Matrix<f32, U3, U1, S>, axis: & Matrix<f32, U3, U1, T>, angle: f32) {
    let (sin_v, cos_v) = angle.sin_cos();
    let cross1 = v.cross(axis);
    let cross2 = axis.cross(&cross1);
    let dot = v.dot(axis);

    v.copy_from(axis);
    v.scale_mut(dot);
    v.add_assign(&cross2.scale(cos_v));
    v.add_assign(&cross1.scale(sin_v));
    v.normalize_mut();
}


fn rotate_matrix_around_axis<T: Storage<f32, U3, U1>>(v: & mut Matrix3<f32>, axis: & Matrix<f32, U3, U1, T>, angle: f32) {
    rotate_vector_around_axis(& mut v.column_mut(0), axis, angle);
    rotate_vector_around_axis(& mut v.column_mut(1), axis, angle);
    rotate_vector_around_axis(& mut v.column_mut(2), axis, angle);
}

fn normal_or_zero(v: & Vector3<f32>) -> Vector3<f32> {
    if let Some (r) = v.try_normalize(0.0) {
        r
    } else {
        Vector3::new (0.0, 0.0, 0.0)
    }
}

fn normal_or_zero_mut(v: & mut Vector3<f32>) {
    let res = v.try_normalize_mut(0.0);
    if res.is_none() {
        v[0] = 0.0;
        v[1] = 0.0;
        v[2] = 0.0;
    }

}
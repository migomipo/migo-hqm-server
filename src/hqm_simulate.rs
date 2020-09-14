use crate::{HQMServer, HQMGameObject, HQMSkater, HQMBody};
use nalgebra::{Vector3, Rotation3, Matrix3, U3, U1, Matrix, ComplexField, Vector2, Point3};
use std::cmp::{min, max};
use std::ops::{Sub, AddAssign};
use nalgebra::base::storage::{Storage, StorageMut};

const GRAVITY: f32 = 0.000680;
impl HQMServer {


    pub(crate) fn simulate_step (&mut self) {

        for p in self.game.objects.iter_mut() {
            if let HQMGameObject::Player (player) = p {
                update_player(player);
                let pos_delta_copy = player.body.pos_delta.clone_owned();
                let rot_axis_copy = player.body.rot_axis.clone_owned();
                update_player2(player);
                update_stick(player, & pos_delta_copy, & rot_axis_copy);
                player.old_input = player.input.clone();

            }
        }
        for p in self.game.objects.iter_mut() {
            if let HQMGameObject::Player (player) = p {
                for i in 1..10 {
                    player.stick_pos += player.stick_pos_delta.scale(0.1);
                }
            }
        }

    }
}

fn update_player(player: & mut HQMSkater) {
    player.body.pos += &player.body.pos_delta;
    player.body.pos_delta[1] -= GRAVITY;
    let feet_pos = &player.body.pos - player.body.rot.column(1).scale(player.height);
    if feet_pos[1] < 0.0 {
        let fwbw_from_client = player.input.fwbw;

        if fwbw_from_client > 0.0 {
            let col2 = player.body.rot.column(2);
            let mut skate_direction = -col2.clone_owned();
            skate_direction[1] = 0.0;
            skate_direction.normalize_mut();
            skate_direction.scale_mut(0.05);
            skate_direction -= &player.body.pos_delta;
            let max_acceleration = if player.body.pos_delta.dot(&col2) > 0.0 {
                0.00055555f32
            } else {
                0.000208f32
            };
            player.body.pos_delta += limit_vector_length(&skate_direction, max_acceleration);
        } else if fwbw_from_client < 0.0 {
            let col2 = player.body.rot.column(2);
            let mut skate_direction = col2.clone_owned();
            skate_direction[1] = 0.0;
            skate_direction.normalize_mut();
            skate_direction.scale_mut(0.05);
            skate_direction -= &player.body.pos_delta;
            let vector_length_limit = if player.body.pos_delta.dot(&col2) < 0.0 {
                0.00055555f32
            } else {
                0.000208f32
            };
            player.body.pos_delta += limit_vector_length(&skate_direction, vector_length_limit);
        }
        if player.input.jump() && !player.old_input.jump() {
            player.body.pos_delta[1] += 0.025;
        }
    }

    let turn = clamp(player.input.turn, -1.0, 1.0);
    if turn != 0.0 {
        let mut column = player.body.rot.column(1).clone_owned();
        column.scale_mut(turn * 6.0 / 14400.0);
        player.body.rot_axis += column;
    }
    // Turn player
    if player.body.rot_axis.norm() > 0.00001 {
        rotate_matrix_around_axis(& mut player.body.rot, &player.body.rot_axis.normalize(), player.body.rot_axis.norm());
    }
    adjust_head_body_rot(& mut player.head_rot, player.input.head_rot);
    adjust_head_body_rot(& mut player.body_rot, player.input.body_rot);


}

fn update_player2 (player: & mut HQMSkater) {
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

        let mut temp2 = unit_y.scale(temp1) - player.body.pos_delta.scale(0.25);
        if temp2.dot(&unit_y) > 0.0 {
            let mut temp_v2 = player.body.rot.column(2).clone_owned();
            temp_v2[1] = 0.0;
            normal_or_zero_mut(& mut temp_v2);

            temp2 -= temp_v2.scale(temp2.dot(&temp_v2));
            limit_rejection(& mut temp2, & unit_y, 1.2);
            player.body.pos_delta += temp2;
            touches_ice = true;
        }
    }
    if player.body.pos[1] < 0.5 && player.body.pos_delta.norm() < 0.025 {
        player.body.pos_delta[1] += 0.00055555555;
        touches_ice = true;
    }
    if touches_ice {
        // This is where the leaning happens
        player.body.rot_axis.scale_mut(0.975);
        let mut unit: Vector3<f32> = Vector3::y();
        let temp = -player.body.pos_delta.dot(&player.body.rot.column(2)) / 0.05;
        rotate_vector_around_axis(& mut unit, &player.body.rot.column(2), 0.225 * turn * temp);

        let mut temp2 = unit.cross(&player.body.rot.column(1));

        let temp2n = normal_or_zero(&temp2);
        temp2.scale_mut(0.008333333);

        let temp3 = -0.25 * temp2n.dot (&player.body.rot_axis);
        temp2 += temp2n.scale(temp3);
        temp2 = limit_vector_length(&temp2, 0.000347);
        player.body.rot_axis += temp2;
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

    let momentum = momentum_stuff(& temp_pos2, & player.body.pos, old_pos_delta, old_rot_axis);
    let stick_pos_movement = 0.125 * (temp_pos2 - &player.stick_pos) - player.stick_pos_delta.scale(0.5) + momentum.scale(0.5);

    player.stick_pos_delta += stick_pos_movement.scale(0.996);
    update_object_stuff(& mut player.body, & stick_pos_movement.scale(-0.004), & temp_pos2)

}

fn update_object_stuff (body: & mut HQMBody, change: & Vector3<f32>, point: & Point3<f32>) {
    let diff1 = point - &body.pos;
    body.pos_delta += change;
    let cross = change.cross(& diff1);
    body.rot_axis += &body.rot * (body.rot.transpose() * cross).component_mul(& body.rot_mul);
}

fn momentum_stuff(p: & Point3<f32>, pos: & Point3<f32>, old_pos_delta: & Vector3<f32>, old_rot_axis: & Vector3<f32>) -> Vector3<f32> {
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
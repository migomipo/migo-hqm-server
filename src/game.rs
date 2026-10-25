use crate::protocol;

use crate::game::RinkSideOfLine::{BlueSide, On, RedSide};
use crate::game::RulesState::Regular;
use crate::protocol::{PuckPacket, SkaterPacket};
use glam::{Vec2, Vec3};
use glamx::Rot3;
use std::f32::consts::PI;
use std::fmt;
use std::fmt::{Display, Formatter};
use std::sync::LazyLock;

/// A half-turn around the Y axis, such as for facing the other end of the rink.
///
/// Use this instead of `Rot3::from_rotation_y(PI)`, which is slightly off as π is rounded in f32,
/// so that it does not turn directions exactly onto the axes.
pub const HALF_TURN_Y: Rot3 = Rot3::from_xyzw(0.0, 1.0, 0.0, 0.0);

/// Various time and scoreboard-related values.
///
/// These values are sent to the client and used to show period, time left, red and blue score,
/// the big "Game Over" text and offside and icing calls.
#[derive(Copy, Clone, Debug)]
pub struct ScoreboardValues {
    pub rules_state: RulesState,

    pub red_score: u32,
    pub blue_score: u32,
    pub period: u32,
    pub time: u32,
    pub goal_message_timer: u32,

    pub game_over: bool,
}

impl Default for ScoreboardValues {
    fn default() -> Self {
        ScoreboardValues {
            rules_state: RulesState::Regular {
                offside_warning: false,
                icing_warning: false,
            },
            red_score: 0,
            blue_score: 0,
            period: 0,
            time: 30000,
            goal_message_timer: 0,
            game_over: false,
        }
    }
}

/// Physics properties that are used for player and puck movement in the physics engine.
#[derive(Debug, Clone)]
pub struct PhysicsConfiguration {
    pub gravity: f32,
    pub limit_jump_speed: bool,
    pub player_acceleration: f32,
    pub player_deceleration: f32,
    pub max_player_speed: f32,
    pub puck_rink_friction: f32,
    pub player_turning: f32,
    pub player_shift_acceleration: f32,
    pub max_player_shift_speed: f32,
    pub player_shift_turning: f32,
}

impl Default for PhysicsConfiguration {
    fn default() -> Self {
        Self {
            gravity: 0.000680555,
            limit_jump_speed: false,
            player_acceleration: 0.000208333,
            player_deceleration: 0.000555555,
            max_player_speed: 0.05,
            puck_rink_friction: 0.05,
            player_turning: 0.00041666666,
            player_shift_acceleration: 0.00027777,
            max_player_shift_speed: 0.0333333,
            player_shift_turning: 0.00038888888,
        }
    }
}

/// Represents a line in the HQM rink.
#[derive(Debug, Clone)]
pub struct RinkLine {
    /// Z coordinate of middle of line.
    pub z: f32,
    /// Width of line.
    pub width: f32,
}

impl RinkLine {
    /// Which side of the line a sphere with the given radius is on. It is on the line if any part
    /// of it touches the line.
    pub fn side_of_line(&self, pos: Vec3, radius: f32) -> RinkSideOfLine {
        let dot = pos.z - self.z;
        if dot > (self.width / 2.0) + radius {
            RedSide
        } else if dot < -(self.width / 2.0) - radius {
            BlueSide
        } else {
            On
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum RinkSideOfLine {
    BlueSide,
    On,
    RedSide,
}

/// A post of a net, as a capsule from `start` to `start + direction`.
#[derive(Debug, Clone)]
pub(crate) struct NetPost {
    pub(crate) start: Vec3,
    pub(crate) direction: Vec3,
    pub(crate) length_squared: f32,
    pub(crate) radius: f32,
}

impl NetPost {
    fn new(start: Vec3, end: Vec3, radius: f32) -> Self {
        let direction = end - start;
        NetPost {
            start,
            direction,
            length_squared: direction.length_squared(),
            radius,
        }
    }
}

/// A surface of a net, with its corners in order and its normal.
#[derive(Debug, Clone)]
pub(crate) struct NetSurface {
    pub(crate) corners: (Vec3, Vec3, Vec3, Vec3),
    pub(crate) normal: Vec3,
}

impl NetSurface {
    fn new(corners: (Vec3, Vec3, Vec3, Vec3)) -> Self {
        let normal = (corners.3 - corners.0)
            .cross(corners.1 - corners.0)
            .normalize();
        NetSurface { corners, normal }
    }
}

/// A rink net.
#[derive(Debug, Clone)]
pub(crate) struct RinkNet {
    pub(crate) posts: Vec<NetPost>,
    pub(crate) surfaces: Vec<NetSurface>,
    /// The corners of a box that holds the whole net, posts included.
    pub(crate) bounds_min: Vec3,
    pub(crate) bounds_max: Vec3,
    pub(crate) left_post: Vec3,
    pub(crate) right_post: Vec3,
    pub(crate) normal: Vec3,
    pub(crate) left_post_inside: Vec3,
    pub(crate) right_post_inside: Vec3,
}

impl RinkNet {
    fn new(pos: Vec3, rot: Rot3) -> Self {
        let front_width = 3.0;
        let back_width = 2.5;
        let front_half_width = front_width / 2.0;
        let back_half_width = back_width / 2.0;
        let height = 1.0;
        let upper_depth = 0.75;
        let lower_depth = 1.0;

        let (
            front_upper_left,
            front_upper_right,
            front_lower_left,
            front_lower_right,
            back_upper_left,
            back_upper_right,
            back_lower_left,
            back_lower_right,
        ) = (
            pos + rot * Vec3::new(-front_half_width, height, 0.0),
            pos + rot * Vec3::new(front_half_width, height, 0.0),
            pos + rot * Vec3::new(-front_half_width, 0.0, 0.0),
            pos + rot * Vec3::new(front_half_width, 0.0, 0.0),
            pos + rot * Vec3::new(-back_half_width, height, -upper_depth),
            pos + rot * Vec3::new(back_half_width, height, -upper_depth),
            pos + rot * Vec3::new(-back_half_width, 0.0, -lower_depth),
            pos + rot * Vec3::new(back_half_width, 0.0, -lower_depth),
        );

        let posts = vec![
            NetPost::new(front_lower_right, front_upper_right, 0.1875),
            NetPost::new(front_lower_left, front_upper_left, 0.1875),
            NetPost::new(front_upper_right, front_upper_left, 0.125),
            NetPost::new(front_lower_left, back_lower_left, 0.125),
            NetPost::new(front_lower_right, back_lower_right, 0.125),
            NetPost::new(front_upper_left, back_upper_left, 0.125),
            NetPost::new(back_upper_right, front_upper_right, 0.125),
            NetPost::new(back_lower_left, back_upper_left, 0.125),
            NetPost::new(back_lower_right, back_upper_right, 0.125),
            NetPost::new(back_lower_left, back_lower_right, 0.125),
            NetPost::new(back_upper_left, back_upper_right, 0.125),
        ];
        let corners = [
            front_upper_left,
            front_upper_right,
            front_lower_left,
            front_lower_right,
            back_upper_left,
            back_upper_right,
            back_lower_left,
            back_lower_right,
        ];
        let max_post_radius = posts.iter().map(|post| post.radius).fold(0.0, f32::max);
        let bounds_min = corners.into_iter().fold(Vec3::INFINITY, Vec3::min) - max_post_radius;
        let bounds_max = corners.into_iter().fold(Vec3::NEG_INFINITY, Vec3::max) + max_post_radius;

        RinkNet {
            posts,
            surfaces: vec![
                NetSurface::new((
                    back_upper_left,
                    back_upper_right,
                    back_lower_right,
                    back_lower_left,
                )),
                NetSurface::new((
                    front_upper_left,
                    back_upper_left,
                    back_lower_left,
                    front_lower_left,
                )),
                NetSurface::new((
                    front_upper_right,
                    front_lower_right,
                    back_lower_right,
                    back_upper_right,
                )),
                NetSurface::new((
                    front_upper_left,
                    front_upper_right,
                    back_upper_right,
                    back_upper_left,
                )),
            ],
            bounds_min,
            bounds_max,
            left_post: front_lower_left,
            right_post: front_lower_right,
            normal: rot * Vec3::Z,
            left_post_inside: rot * Vec3::X,
            right_post_inside: rot * Vec3::NEG_X,
        }
    }
}

/// A rink, with collision boundaries and nets.
///
/// In HQM, all coordinates are based in meters.
///
/// The X axis goes along the length of the rink,
/// which is 61 meters long by default. The coordinates starts with 0.0 on the wall closest to the blue net, a
/// and goes all the way up to 61.0 at the wall at the other end.
///
/// The Z axis goes along the width of the rink, which is 30 meters wide by default. The coordinates start with
/// 0.0 at the left wall looking from the position of the red goalie, and goes up to 30.0 at the right wall.
#[derive(Debug, Clone)]
pub struct Rink {
    pub(crate) planes: Vec<(Vec3, Vec3)>,
    pub(crate) corners: Vec<(Vec3, Vec3, f32)>,
    pub(crate) red_net: RinkNet,
    pub(crate) blue_net: RinkNet,
    pub center_line: RinkLine,
    pub red_zone_blue_line: RinkLine,
    pub blue_zone_blue_line: RinkLine,
    pub width: f32,
    pub length: f32,
}

impl Rink {
    pub(crate) fn new(width: f32, length: f32, corner_radius: f32) -> Self {
        let planes = vec![
            (Vec3::ZERO, Vec3::Y),
            (Vec3::new(0.0, 0.0, length), Vec3::NEG_Z),
            (Vec3::ZERO, Vec3::Z),
            (Vec3::new(width, 0.0, 0.0), Vec3::NEG_X),
            (Vec3::ZERO, Vec3::X),
        ];
        let r = corner_radius;
        let wr = width - corner_radius;
        let lr = length - corner_radius;
        let corners = vec![
            (
                Vec3::new(r, 0.0, r),
                Vec3::new(-1.0, 0.0, -1.0),
                corner_radius,
            ),
            (
                Vec3::new(wr, 0.0, r),
                Vec3::new(1.0, 0.0, -1.0),
                corner_radius,
            ),
            (
                Vec3::new(wr, 0.0, lr),
                Vec3::new(1.0, 0.0, 1.0),
                corner_radius,
            ),
            (
                Vec3::new(r, 0.0, lr),
                Vec3::new(-1.0, 0.0, 1.0),
                corner_radius,
            ),
        ];

        let line_width = 0.3; // IIHF rule 17iii, 17iv
        let goal_line_distance = 4.0; // IIHF rule 17iv

        let blue_line_distance_neutral_zone_edge = 22.86;
        let blue_line_distance_mid = blue_line_distance_neutral_zone_edge - line_width / 2.0; // IIHF rule 17v and 17vi
        // IIHF specifies distance between end boards and edge closest to the neutral zone, but my code specifies middle of line

        let center_x = width / 2.0;

        let red_zone_blueline_z = length - blue_line_distance_mid;
        let center_z = length / 2.0;
        let blue_zone_blueline_z = blue_line_distance_mid;

        let blue_net = RinkNet::new(Vec3::new(center_x, 0.0, goal_line_distance), Rot3::IDENTITY);
        let red_net = RinkNet::new(
            Vec3::new(center_x, 0.0, length - goal_line_distance),
            HALF_TURN_Y,
        );

        let red_zone_blue_line = RinkLine {
            z: red_zone_blueline_z,
            width: line_width,
        };
        let blue_zone_blue_line = RinkLine {
            z: blue_zone_blueline_z,
            width: line_width,
        };
        let center_line = RinkLine {
            z: center_z,
            width: line_width,
        };

        Rink {
            planes,
            corners,
            red_net,
            blue_net,
            center_line,
            red_zone_blue_line,
            blue_zone_blue_line,
            width,
            length,
        }
    }
}

/// Represents a physical body (both players and pucks) with a position, rotation and linear and angular velocities.
#[derive(Debug, Clone)]
pub struct PhysicsBody {
    pub pos: Vec3,              // Measured in meters
    pub linear_velocity: Vec3,  // Measured in meters per hundred of a second
    pub rot: Rot3,              // Rotation matrix
    pub angular_velocity: Vec3, // Measured in radians per hundred of a second
    pub(crate) inv_moment_of_inertia: Vec3,
}

impl PhysicsBody {
    pub fn apply_acceleration_to_object(&mut self, change: Vec3, point: Vec3) {
        self.linear_velocity += change;
        let lever = point - self.pos;
        let torque = lever.cross(change);
        self.angular_velocity +=
            self.rot * ((self.rot.inverse() * torque) * self.inv_moment_of_inertia);
    }
}

/// Represents a skater object.
///
/// If you set the position, rotation, and/or linear velocity directly without adjusting the collision balls,
/// some weird things will happen with the inertia of the player. To fix this, use the [SkaterObject::reset_collision_balls] method after
/// changing the physics properties. To move a skater somewhere else, use [SkaterObject::teleport],
/// which also takes care of the stick.
#[derive(Debug, Clone)]
pub struct SkaterObject {
    pub body: PhysicsBody,
    /// Stick position in absolute space, measured in meters.
    pub stick_pos: Vec3,
    /// Stick velocity, measured in meters per hundred of a second
    pub stick_velocity: Vec3,
    /// Stick rotation.
    pub stick_rot: Rot3, // Rotation matrix
    /// Left-right body rotation around the Y axis in radians. Left is negative and right is positive, and the normal range is -(7/8)π to (7/8)π.
    pub head_rot: f32, // Radians
    /// Forward-backward body rotation around the X axis in radians. Backwards is negative and forwards is positive, and the normal range is -π/2 to π/2.
    pub body_rot: f32, // Radians
    pub(crate) height: f32,
    pub(crate) jumped_last_frame: bool,
    pub stick_placement: Vec2,       // Azimuth and inclination in radians
    pub stick_placement_delta: Vec2, // Change in azimuth and inclination per hundred of a second
    pub collision_balls: Vec<SkaterCollisionBall>,
    pub hand: SkaterHand,
}

impl SkaterObject {
    pub fn new(pos: Vec3, rot: Rot3, hand: SkaterHand) -> Self {
        let linear_velocity = Vec3::ZERO;
        let collision_balls = SkaterObject::get_collision_balls(&pos, &rot, &linear_velocity, 1.0);
        SkaterObject {
            body: PhysicsBody {
                pos,
                linear_velocity,
                rot,
                angular_velocity: Vec3::ZERO,
                inv_moment_of_inertia: Vec3::new(2.75, 6.16, 2.35),
            },
            stick_pos: pos,
            stick_velocity: Vec3::ZERO,
            stick_rot: Rot3::IDENTITY,
            head_rot: 0.0,
            body_rot: 0.0,
            height: 0.75,
            jumped_last_frame: false,
            stick_placement: Vec2::ZERO,
            stick_placement_delta: Vec2::ZERO,
            hand,
            collision_balls,
        }
    }

    /// Returns the position of the bottom of the skater.
    pub fn feet_position(&self) -> Vec3 {
        self.body.pos - self.body.rot * Vec3::Y * self.height
    }

    /// Moves the skater to a new position and rotation, and stops it.
    ///
    /// The stick moves along with the skater, so it stays in the same place relative to the
    /// skater, and the collision balls are reset. Head, body and stick placement are kept.
    ///
    /// To have the skater keep moving from the new position, set the velocities afterwards and
    /// call [SkaterObject::reset_collision_balls].
    pub fn teleport(&mut self, pos: Vec3, rot: Rot3) {
        let frame_change = rot * self.body.rot.inverse();
        self.stick_pos = pos + frame_change * (self.stick_pos - self.body.pos);
        self.stick_rot = frame_change * self.stick_rot;
        self.stick_velocity = Vec3::ZERO;
        self.body.pos = pos;
        self.body.rot = rot;
        self.body.linear_velocity = Vec3::ZERO;
        self.body.angular_velocity = Vec3::ZERO;
        self.reset_collision_balls();
    }

    pub fn reset_collision_balls(&mut self) {
        self.collision_balls = Self::get_collision_balls(
            &self.body.pos,
            &self.body.rot,
            &self.body.linear_velocity,
            1.0,
        );
    }
    fn get_collision_balls(
        pos: &Vec3,
        rot: &Rot3,
        linear_velocity: &Vec3,
        mass: f32,
    ) -> Vec<SkaterCollisionBall> {
        vec![
            SkaterCollisionBall::from_skater(
                Vec3::new(0.0, 0.0, 0.0),
                pos,
                rot,
                linear_velocity,
                0.225,
                mass,
            ),
            SkaterCollisionBall::from_skater(
                Vec3::new(0.25, 0.3125, 0.0),
                pos,
                rot,
                linear_velocity,
                0.25,
                mass,
            ),
            SkaterCollisionBall::from_skater(
                Vec3::new(-0.25, 0.3125, 0.0),
                pos,
                rot,
                linear_velocity,
                0.25,
                mass,
            ),
            SkaterCollisionBall::from_skater(
                Vec3::new(-0.1875, -0.1875, 0.0),
                pos,
                rot,
                linear_velocity,
                0.1875,
                mass,
            ),
            SkaterCollisionBall::from_skater(
                Vec3::new(0.1875, -0.1875, 0.0),
                pos,
                rot,
                linear_velocity,
                0.1875,
                mass,
            ),
            SkaterCollisionBall::from_skater(
                Vec3::new(0.0, 0.5, 0.0),
                pos,
                rot,
                linear_velocity,
                0.1875,
                mass,
            ),
        ]
    }

    pub(crate) fn get_packet(&self) -> SkaterPacket {
        let rot = protocol::convert_matrix_to_network(31, &self.body.rot);
        let stick_rot = protocol::convert_matrix_to_network(25, &self.stick_rot);

        SkaterPacket {
            pos: (
                get_position(17, 1024.0 * self.body.pos.x),
                get_position(17, 1024.0 * self.body.pos.y),
                get_position(17, 1024.0 * self.body.pos.z),
            ),
            rot,
            stick_pos: (
                get_position(13, 1024.0 * (self.stick_pos.x - self.body.pos.x + 4.0)),
                get_position(13, 1024.0 * (self.stick_pos.y - self.body.pos.y + 4.0)),
                get_position(13, 1024.0 * (self.stick_pos.z - self.body.pos.z + 4.0)),
            ),
            stick_rot,
            head_rot: get_position(16, (self.head_rot + 2.0) * 8192.0),
            body_rot: get_position(16, (self.body_rot + 2.0) * 8192.0),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SkaterCollisionBall {
    pub offset: Vec3,
    pub pos: Vec3,
    pub velocity: Vec3,
    pub radius: f32,
    pub mass: f32,
}

impl SkaterCollisionBall {
    fn from_skater(
        offset: Vec3,
        skater_pos: &Vec3,
        skater_rot: &Rot3,
        velocity: &Vec3,
        radius: f32,
        mass: f32,
    ) -> Self {
        let pos = skater_pos + skater_rot * offset;
        SkaterCollisionBall {
            offset,
            pos,
            velocity: *velocity,
            radius,
            mass,
        }
    }
}

/// Key and mouse inputs sent from the client to the server.
///
#[derive(Debug, Clone)]
pub struct PlayerInput {
    /// Stick angle. Normal range is -1 to 1.
    pub stick_angle: f32,
    /// Left or right turning. Negative is left, positive is right. Normal range is -1 to 1.
    pub turn: f32,
    /// Forward or backward movement. Negative is backwards, positive is forwards. Normal range is -1 to 1.
    pub fwbw: f32,
    /// Stick position.
    ///
    /// The position is a vector with a X axis and a Y axis value, both in radians.
    /// For the X axis, left is negative and right is positive, and the normal range is -π/2 to π/2.
    /// For the Y axis, down is negative and up is positive, and the normal range is -(5/16)π to π/8
    pub stick: Vec2,

    /// Left-right body rotation around the Y axis in radians. Left is negative and right is positive, and the normal range is -(7/8)π to (7/8)π.
    pub head_rot: f32,

    /// Forward-backward body rotation around the X axis in radians. Backwards is negative and forwards is positive, and the normal range is -π/2 to π/2.
    pub body_rot: f32,

    /// Key bit mask.
    /// Some utility methods are provided to check whether certain keys are pressed.
    pub keys: u32,
}

impl Default for PlayerInput {
    fn default() -> Self {
        PlayerInput {
            stick_angle: 0.0,
            turn: 0.0,
            fwbw: 0.0,
            stick: Vec2::ZERO,
            head_rot: 0.0,
            body_rot: 0.0,
            keys: 0,
        }
    }
}

impl PlayerInput {
    pub fn jump(&self) -> bool {
        self.keys & 0x1 != 0
    }
    pub fn crouch(&self) -> bool {
        self.keys & 0x2 != 0
    }
    pub fn join_red(&self) -> bool {
        self.keys & 0x4 != 0
    }
    pub fn join_blue(&self) -> bool {
        self.keys & 0x8 != 0
    }
    pub fn shift(&self) -> bool {
        self.keys & 0x10 != 0
    }
    pub fn spectate(&self) -> bool {
        self.keys & 0x20 != 0
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum SkaterHand {
    Left,
    Right,
}

/// Represents an HQM puck.
#[derive(Debug, Clone)]
pub struct Puck {
    pub body: PhysicsBody,
    pub radius: f32,
    pub height: f32,
}

impl Puck {
    pub fn new(pos: Vec3, rot: Rot3) -> Self {
        Puck {
            body: PhysicsBody {
                pos,
                linear_velocity: Vec3::ZERO,
                rot,
                angular_velocity: Vec3::ZERO,
                inv_moment_of_inertia: Vec3::new(223.5, 128.0, 223.5),
            },
            radius: 0.125,
            height: 0.041_25,
        }
    }

    pub(crate) fn get_packet(&self) -> PuckPacket {
        let rot = protocol::convert_matrix_to_network(31, &self.body.rot);
        PuckPacket {
            pos: (
                get_position(17, 1024.0 * self.body.pos.x),
                get_position(17, 1024.0 * self.body.pos.y),
                get_position(17, 1024.0 * self.body.pos.z),
            ),
            rot,
        }
    }

    pub(crate) fn get_puck_vertices(&self) -> [Vec3; 48] {
        // Computed at run time, as the vertices were before the table, so the values stay exactly
        // the same
        static ANGLES: LazyLock<[(f32, f32); 16]> =
            LazyLock::new(|| std::array::from_fn(|i| ((i as f32) * PI / 8.0).sin_cos()));
        let mut res = [Vec3::ZERO; 48];
        for (i, &(sin, cos)) in ANGLES.iter().enumerate() {
            for j in -1..=1 {
                let point = Vec3::new(
                    cos * self.radius,
                    (j as f32) * self.height,
                    sin * self.radius,
                );
                let point2 = self.body.rot * point;
                let index = i as i32 * 3 + 1 - j;
                res[index as usize] = self.body.pos + point2;
            }
        }
        res
    }
}

/// Rules state sent to the client.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum RulesState {
    Regular {
        offside_warning: bool,
        icing_warning: bool,
    },
    Offside,
    Icing,
}

impl Default for RulesState {
    fn default() -> Self {
        Regular {
            offside_warning: false,
            icing_warning: false,
        }
    }
}

fn get_position(bits: u32, v: f32) -> u32 {
    let temp = v as i32;
    if temp < 0 {
        0
    } else if temp > ((1 << bits) - 1) {
        ((1 << bits) - 1) as u32
    } else {
        temp as u32
    }
}

/**
Player slot index.

This simply represents the player "slot" that each player has in the player list, and thus two
different users might have had the same index since the server started. Two players can
never have the same index at the same time. This is mostly useful for handling chat commands that
take a user number as input.
 **/
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub struct PlayerIndex(pub(crate) usize);

/**
Unique player ID.

This ID will be unique for each player session while this server running. That is, each player
who connects will always get a new ID that no other user has had since the server started. A player who disconnects
and then reconnects will get a different Id.
**/
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub struct PlayerId {
    pub(crate) index: PlayerIndex,
    pub(crate) counter: u32,
}

impl std::fmt::Display for PlayerIndex {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for PlayerIndex {
    type Err = std::num::ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse().map(PlayerIndex)
    }
}

impl std::fmt::Display for PlayerId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.index.0, self.counter)
    }
}

/**
Unique puck ID.

Each puck that is spawned gets an ID that no other puck has had since the server started. An ID
of a puck that has been removed never refers to a puck that was spawned later in the same object
slot.
**/
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub struct PuckId {
    pub(crate) index: usize,
    pub(crate) counter: u32,
}

impl std::fmt::Display for PuckId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.index, self.counter)
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub enum Team {
    Red,
    Blue,
}

impl Team {
    pub(crate) fn get_num(self) -> u32 {
        match self {
            Team::Red => 0,
            Team::Blue => 1,
        }
    }

    pub fn get_other_team(self) -> Self {
        match self {
            Team::Red => Team::Blue,
            Team::Blue => Team::Red,
        }
    }
}

impl Display for Team {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Team::Red => write!(f, "Red"),
            Team::Blue => write!(f, "Blue"),
        }
    }
}

#[derive(Debug, Copy, Clone)]
pub enum PhysicsEvent {
    PuckTouch { player: PlayerId, puck: PuckId },
    PuckReachedDefensiveLine { team: Team, puck: PuckId },
    PuckPassedDefensiveLine { team: Team, puck: PuckId },
    PuckReachedCenterLine { team: Team, puck: PuckId },
    PuckPassedCenterLine { team: Team, puck: PuckId },
    PuckReachedOffensiveZone { team: Team, puck: PuckId },
    PuckEnteredOffensiveZone { team: Team, puck: PuckId },

    PuckEnteredNet { team: Team, puck: PuckId },
    PuckPassedGoalLine { team: Team, puck: PuckId },
    PuckTouchedNet { team: Team, puck: PuckId },
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::FRAC_PI_2;

    #[test]
    fn half_turn_turns_the_axes_exactly() {
        assert_eq!(HALF_TURN_Y * Vec3::X, Vec3::NEG_X);
        assert_eq!(HALF_TURN_Y * Vec3::Y, Vec3::Y);
        assert_eq!(HALF_TURN_Y * Vec3::Z, Vec3::NEG_Z);
        assert!(HALF_TURN_Y.abs_diff_eq(Rot3::from_rotation_y(PI), 1e-7));
    }

    #[test]
    fn red_net_is_the_blue_net_turned_half_around_the_rink_center() {
        let rink = Rink::new(30.0, 61.0, 8.5);
        let (red, blue) = (&rink.red_net, &rink.blue_net);
        let turn_point = |v: Vec3| Vec3::new(30.0 - v.x, v.y, 61.0 - v.z);
        let turn_direction = |v: Vec3| Vec3::new(-v.x, v.y, -v.z);

        assert_eq!(blue.normal, Vec3::Z);
        assert_eq!(red.normal, Vec3::NEG_Z);
        assert_eq!(red.left_post_inside, Vec3::NEG_X);
        assert_eq!(red.right_post_inside, Vec3::X);
        assert_eq!(red.left_post, turn_point(blue.left_post));
        assert_eq!(red.right_post, turn_point(blue.right_post));
        for (red, blue) in red.posts.iter().zip(&blue.posts) {
            assert_eq!(red.start, turn_point(blue.start));
            assert_eq!(red.direction, turn_direction(blue.direction));
            assert_eq!(red.length_squared, blue.length_squared);
        }
        for (red, blue) in red.surfaces.iter().zip(&blue.surfaces) {
            let (a, b, c, d) = blue.corners;
            let corners = (turn_point(a), turn_point(b), turn_point(c), turn_point(d));
            assert_eq!(red.corners, corners);
            assert_eq!(red.normal, turn_direction(blue.normal));
        }
    }

    #[test]
    fn net_bounds_hold_the_whole_net_and_its_posts() {
        let net = &Rink::new(30.0, 61.0, 8.5).blue_net;
        assert_eq!(net.bounds_min, Vec3::new(13.5, 0.0, 3.0) - 0.1875);
        assert_eq!(net.bounds_max, Vec3::new(16.5, 1.0, 4.0) + 0.1875);
    }

    #[test]
    fn both_sides_of_a_line_start_at_its_edges() {
        let line = RinkLine {
            z: 10.0,
            width: 0.3,
        };
        let at = |z: f32, radius| line.side_of_line(Vec3::new(5.0, 0.0, z), radius);

        // The edges are half the line width from the middle on both sides.
        for (offset, side) in [(0.16, RedSide), (0.14, On), (-0.14, On), (-0.16, BlueSide)] {
            assert_eq!(at(10.0 + offset, 0.0), side, "offset {offset}");
        }
        // A sphere is on the line while any part of it touches it.
        assert_eq!(at(10.3, 0.1), RedSide);
        assert_eq!(at(10.2, 0.1), On);
        assert_eq!(at(9.8, 0.1), On);
        assert_eq!(at(9.7, 0.1), BlueSide);
    }

    #[test]
    fn teleport_moves_the_stick_along_and_stops_the_skater() {
        let mut skater =
            SkaterObject::new(Vec3::new(5.0, 1.0, 10.0), Rot3::IDENTITY, SkaterHand::Right);
        skater.stick_pos = Vec3::new(5.0, 0.2, 11.5);
        skater.stick_rot = Rot3::from_rotation_x(0.3);
        skater.stick_velocity = Vec3::X;
        skater.stick_placement = Vec2::new(0.5, -0.2);
        skater.head_rot = 0.4;
        skater.body.linear_velocity = Vec3::new(0.1, 0.0, 0.2);
        skater.body.angular_velocity = Vec3::Y;
        skater.reset_collision_balls();

        let pos = Vec3::new(20.0, 1.5, 40.0);
        let rot = Rot3::from_rotation_y(FRAC_PI_2);
        skater.teleport(pos, rot);

        assert_eq!(skater.body.pos, pos);
        assert!(skater.body.rot.abs_diff_eq(rot, 1e-6));
        // A quarter turn around Y turns the stick's offset of 1.5 m along Z into 1.5 m along X.
        let stick_offset = skater.stick_pos - pos;
        assert!(stick_offset.abs_diff_eq(Vec3::new(1.5, -0.8, 0.0), 1e-5));
        assert!(
            skater
                .stick_rot
                .abs_diff_eq(rot * Rot3::from_rotation_x(0.3), 1e-6)
        );
        assert_eq!(skater.body.linear_velocity, Vec3::ZERO);
        assert_eq!(skater.body.angular_velocity, Vec3::ZERO);
        assert_eq!(skater.stick_velocity, Vec3::ZERO);
        assert_eq!(skater.stick_placement, Vec2::new(0.5, -0.2));
        assert_eq!(skater.head_rot, 0.4);

        let fresh = SkaterObject::new(pos, rot, SkaterHand::Right);
        assert_eq!(skater.collision_balls.len(), fresh.collision_balls.len());
        for (ball, fresh_ball) in skater.collision_balls.iter().zip(&fresh.collision_balls) {
            assert!(ball.pos.abs_diff_eq(fresh_ball.pos, 1e-5));
            assert_eq!(ball.velocity, Vec3::ZERO);
        }
    }
}

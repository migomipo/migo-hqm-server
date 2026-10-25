//! Tests of the physics simulation.

use super::*;
use crate::game::HALF_TURN_Y;
use crate::server::test_server;
use std::collections::BTreeSet;

/// A server with skaters and pucks on the default rink, for stepping the physics directly.
struct Sim {
    server: HQMServer,
    next_port: u16,
}

impl Sim {
    fn new() -> Self {
        Sim {
            server: test_server(),
            next_port: 1,
        }
    }

    /// Adds a player and spawns their skater.
    fn skater(&mut self, team: Team, pos: Vec3, rot: Rot3) -> PlayerId {
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], self.next_port));
        self.next_port += 1;
        let player_id = self
            .server
            .state
            .player_message_state
            .add_player("skater", addr)
            .unwrap();
        assert!(
            self.server
                .context()
                .spawn_skater(player_id, team, pos, rot, false)
        );
        player_id
    }

    /// Spawns a skater and lets it settle on the ice.
    fn settled_skater(&mut self, pos: Vec3, rot: Rot3) -> PlayerId {
        let player_id = self.skater(Team::Red, pos, rot);
        self.steps(200);
        player_id
    }

    fn puck(&mut self, pos: Vec3, velocity: Vec3) -> PuckId {
        let mut puck = Puck::new(pos, Rot3::IDENTITY);
        puck.body.linear_velocity = velocity;
        self.server.context().objects.spawn_puck(puck).unwrap()
    }

    /// Spawns a puck on the ice, sliding with the given velocity.
    fn sliding_puck(&mut self, x: f32, z: f32, velocity: Vec3) -> PuckId {
        self.puck(Vec3::new(x, RESTING_PUCK_HEIGHT, z), velocity)
    }

    fn input(&mut self, player_id: PlayerId) -> &mut PlayerInput {
        &mut self
            .server
            .state
            .player_message_state
            .players
            .get_player_mut(player_id)
            .unwrap()
            .input
    }

    fn step(&mut self) -> Vec<PhysicsEvent> {
        self.server.simulate_step().into_vec()
    }

    fn steps(&mut self, steps: u32) -> Vec<PhysicsEvent> {
        (0..steps).flat_map(|_| self.step()).collect()
    }

    fn get_puck(&self, puck_id: PuckId) -> &Puck {
        self.server
            .state
            .objects
            .iter()
            .find_map(|object| match object {
                Some(GameObject::Puck(id, puck)) if *id == puck_id => Some(puck),
                _ => None,
            })
            .unwrap()
    }

    fn get_puck_mut(&mut self, puck_id: PuckId) -> &mut Puck {
        self.server
            .state
            .objects
            .iter_mut()
            .find_map(|object| match object {
                Some(GameObject::Puck(id, puck)) if *id == puck_id => Some(puck),
                _ => None,
            })
            .unwrap()
    }

    fn pucks(&self) -> impl Iterator<Item = &Puck> {
        self.server
            .state
            .objects
            .iter()
            .filter_map(|object| match object {
                Some(GameObject::Puck(_, puck)) => Some(puck),
                _ => None,
            })
    }

    fn get_skater(&self, player_id: PlayerId) -> &SkaterObject {
        self.server
            .state
            .objects
            .iter()
            .find_map(|object| match object {
                Some(GameObject::Skater(id, skater)) if *id == player_id => Some(skater),
                _ => None,
            })
            .unwrap()
    }

    fn get_skater_mut(&mut self, player_id: PlayerId) -> &mut SkaterObject {
        self.server
            .state
            .objects
            .iter_mut()
            .find_map(|object| match object {
                Some(GameObject::Skater(id, skater)) if *id == player_id => Some(skater),
                _ => None,
            })
            .unwrap()
    }

    /// The state of every object, to compare simulations with.
    fn snapshot(&self) -> String {
        format!("{:?}", self.server.state.objects)
    }
}

/// About the height of the center of a puck lying on the ice.
const RESTING_PUCK_HEIGHT: f32 = 0.037;

const SHIFT_KEY: u32 = 0x10;
const JUMP_KEY: u32 = 0x1;
const CROUCH_KEY: u32 = 0x2;

/// The direction a skater faces, flattened to the ice.
fn facing(skater: &SkaterObject) -> Vec3 {
    let forward = skater.body.rot * Vec3::NEG_Z;
    Vec3::new(forward.x, 0.0, forward.z).normalize()
}

/// The point at the other end of the rink after a half-turn around the center of the rink.
fn turned(v: Vec3) -> Vec3 {
    Vec3::new(30.0 - v.x, v.y, 61.0 - v.z)
}

fn other_team(team: Team) -> Team {
    match team {
        Team::Red => Team::Blue,
        Team::Blue => Team::Red,
    }
}

/// The event as seen from the other end of the rink.
fn with_teams_swapped(event: PhysicsEvent) -> PhysicsEvent {
    use PhysicsEvent::*;
    match event {
        PuckTouch { .. } => event,
        PuckReachedDefensiveLine { team, puck } => PuckReachedDefensiveLine {
            team: other_team(team),
            puck,
        },
        PuckPassedDefensiveLine { team, puck } => PuckPassedDefensiveLine {
            team: other_team(team),
            puck,
        },
        PuckReachedCenterLine { team, puck } => PuckReachedCenterLine {
            team: other_team(team),
            puck,
        },
        PuckPassedCenterLine { team, puck } => PuckPassedCenterLine {
            team: other_team(team),
            puck,
        },
        PuckReachedOffensiveZone { team, puck } => PuckReachedOffensiveZone {
            team: other_team(team),
            puck,
        },
        PuckEnteredOffensiveZone { team, puck } => PuckEnteredOffensiveZone {
            team: other_team(team),
            puck,
        },
        PuckEnteredNet { team, puck } => PuckEnteredNet {
            team: other_team(team),
            puck,
        },
        PuckPassedGoalLine { team, puck } => PuckPassedGoalLine {
            team: other_team(team),
            puck,
        },
        PuckTouchedNet { team, puck } => PuckTouchedNet {
            team: other_team(team),
            puck,
        },
    }
}

fn entered_net(events: &[PhysicsEvent], net_team: Team) -> bool {
    events.iter().any(
        |event| matches!(event, PhysicsEvent::PuckEnteredNet { team, .. } if *team == net_team),
    )
}

fn touched_net(events: &[PhysicsEvent], net_team: Team) -> bool {
    events.iter().any(
        |event| matches!(event, PhysicsEvent::PuckTouchedNet { team, .. } if *team == net_team),
    )
}

fn passed_goal_line(events: &[PhysicsEvent], net_team: Team) -> bool {
    events.iter().any(
        |event| matches!(event, PhysicsEvent::PuckPassedGoalLine { team, .. } if *team == net_team),
    )
}

fn assert_close(actual: Vec3, expected: Vec3, tolerance: f32) {
    assert!(
        actual.abs_diff_eq(expected, tolerance),
        "{actual:?} is not within {tolerance} of {expected:?}"
    );
}

/// A small random number generator, so tests can try many cases and still be repeatable.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 40) as f32) / ((1u64 << 24) as f32)
    }

    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.next()
    }

    fn vec3(&mut self, lo: Vec3, hi: Vec3) -> Vec3 {
        Vec3::new(
            self.range(lo.x, hi.x),
            self.range(lo.y, hi.y),
            self.range(lo.z, hi.z),
        )
    }
}

// Helper functions

#[test]
fn replace_nan_only_replaces_nan() {
    assert_eq!(replace_nan(f32::NAN, 0.5), 0.5);
    assert_eq!(replace_nan(-2.0, 0.5), -2.0);
    assert_eq!(replace_nan(f32::INFINITY, 0.5), f32::INFINITY);
}

#[test]
fn limit_friction_keeps_vectors_inside_the_friction_cone() {
    let mut v = Vec3::new(0.3, 1.0, -0.2);
    limit_friction(&mut v, Vec3::Y, 0.5);
    assert_close(v, Vec3::new(0.3, 1.0, -0.2), 1e-6);
}

#[test]
fn limit_friction_limits_the_part_along_the_surface() {
    let mut v = Vec3::new(2.0, 1.0, 0.0);
    limit_friction(&mut v, Vec3::Y, 0.5);
    assert_close(v, Vec3::new(0.5, 1.0, 0.0), 1e-6);
}

#[test]
fn limit_friction_keeps_vectors_along_the_normal() {
    let mut v = Vec3::new(0.0, -3.0, 0.0);
    limit_friction(&mut v, Vec3::Y, 0.5);
    assert_eq!(v, Vec3::new(0.0, -3.0, 0.0));
}

#[test]
fn head_and_body_rotation_turn_a_limited_amount_per_step() {
    let step = 0.06666667;
    let mut rot = 0.0;
    adjust_head_body_rot(&mut rot, 1.0);
    assert_eq!(rot, step);
    adjust_head_body_rot(&mut rot, -1.0);
    assert_eq!(rot, 0.0);
    adjust_head_body_rot(&mut rot, -1.0);
    assert_eq!(rot, -step);
    adjust_head_body_rot(&mut rot, -0.1);
    assert_eq!(rot, -0.1);
}

#[test]
fn speed_of_a_point_includes_the_rotation() {
    let speed = speed_of_point_including_rotation(
        Vec3::new(2.0, 0.0, 0.0),
        Vec3::new(1.0, 0.0, 0.0),
        Vec3::new(0.0, 0.0, 0.5),
        Vec3::new(0.0, 1.0, 0.0),
    );
    // Turning around Y moves a point on the X axis towards -Z
    assert_close(speed, Vec3::new(0.0, 0.0, -0.5), 1e-6);
}

// Rink collisions

#[test]
fn a_sphere_in_the_middle_of_the_rink_touches_nothing() {
    let rink = Rink::new(30.0, 61.0, 8.5);
    assert!(collision_between_sphere_and_rink(Vec3::new(15.0, 1.0, 30.5), 0.5, &rink).is_none());
}

#[test]
fn a_sphere_below_the_ice_is_pushed_up() {
    let rink = Rink::new(30.0, 61.0, 8.5);
    let (overlap, normal) =
        collision_between_sphere_and_rink(Vec3::new(15.0, 0.1, 30.5), 0.25, &rink).unwrap();
    assert!((overlap - 0.15).abs() < 1e-6);
    assert_eq!(normal, Vec3::Y);
}

#[test]
fn a_sphere_against_the_boards_is_pushed_back_into_the_rink() {
    let rink = Rink::new(30.0, 61.0, 8.5);
    let cases = [
        (Vec3::new(0.1, 1.0, 30.5), Vec3::X),
        (Vec3::new(29.9, 1.0, 30.5), Vec3::NEG_X),
        (Vec3::new(15.0, 1.0, 0.1), Vec3::Z),
        (Vec3::new(15.0, 1.0, 60.9), Vec3::NEG_Z),
    ];
    for (pos, expected_normal) in cases {
        let (overlap, normal) = collision_between_sphere_and_rink(pos, 0.25, &rink).unwrap();
        assert!((overlap - 0.15).abs() < 1e-5, "{pos:?}: {overlap}");
        assert_eq!(normal, expected_normal, "{pos:?}");
    }
}

#[test]
fn the_rink_corners_are_rounded() {
    let rink = Rink::new(30.0, 61.0, 8.5);
    // Far from both walls, but outside the rounded corner
    let pos = Vec3::new(1.0, 1.0, 1.0);
    assert!(collision_between_sphere_and_rink(pos, 0.0, &rink.clone()).is_some());

    // Just touching the corner arc, the sphere is pushed towards the center of the arc
    let corner_center = Vec3::new(8.5, 1.0, 8.5);
    let pos = corner_center + 8.4 * Vec3::new(-1.0, 0.0, -1.0).normalize();
    let (overlap, normal) = collision_between_sphere_and_rink(pos, 0.25, &rink).unwrap();
    assert!((overlap - 0.15).abs() < 1e-4, "{overlap}");
    assert_close(normal, Vec3::new(1.0, 0.0, 1.0).normalize(), 1e-6);
}

// Net collisions

fn test_post() -> NetPost {
    NetPost {
        start: Vec3::ZERO,
        direction: Vec3::Y,
        length_squared: 1.0,
        radius: 0.1,
    }
}

#[test]
fn a_sphere_touching_the_side_of_a_post_is_pushed_away_from_it() {
    let (overlap, normal) =
        collision_between_sphere_and_post(Vec3::new(0.2, 0.5, 0.0), 0.125, &test_post()).unwrap();
    assert!((overlap - 0.025).abs() < 1e-6);
    assert_close(normal, Vec3::X, 1e-6);
}

#[test]
fn a_sphere_beyond_the_end_of_a_post_is_pushed_away_from_the_end() {
    let (overlap, normal) =
        collision_between_sphere_and_post(Vec3::new(0.0, 1.2, 0.0), 0.125, &test_post()).unwrap();
    assert!((overlap - 0.025).abs() < 1e-6);
    assert_close(normal, Vec3::Y, 1e-6);
}

#[test]
fn a_sphere_away_from_a_post_does_not_touch_it() {
    let post = test_post();
    assert!(collision_between_sphere_and_post(Vec3::new(0.3, 0.5, 0.0), 0.125, &post).is_none());
    assert!(collision_between_sphere_and_post(Vec3::new(0.0, 1.3, 0.0), 0.125, &post).is_none());
}

#[test]
fn a_puck_under_the_roof_of_the_net_is_pushed_down() {
    let rink = Rink::new(30.0, 61.0, 8.5);
    let pos = Vec3::new(15.0, 0.95, 3.6);
    let (_, overlap, normal) =
        collision_between_sphere_and_net(pos, 0.125, &rink.blue_net).unwrap();
    assert!((overlap - 0.075).abs() < 1e-6, "{overlap}");
    assert_close(normal, Vec3::NEG_Y, 1e-6);
}

#[test]
fn a_puck_in_front_of_the_net_does_not_touch_it() {
    let rink = Rink::new(30.0, 61.0, 8.5);
    let pos = Vec3::new(15.0, 0.5, 5.0);
    assert!(collision_between_sphere_and_net(pos, 0.125, &rink.blue_net).is_none());
    for post in &rink.blue_net.posts {
        assert!(collision_between_sphere_and_post(pos, 0.125, post).is_none());
    }
}

#[test]
fn a_puck_in_the_net_is_near_the_net() {
    let rink = Rink::new(30.0, 61.0, 8.5);
    assert!(is_near_net(
        Vec3::new(15.0, 0.5, 3.5),
        0.125,
        &rink.blue_net
    ));
    assert!(is_near_net(
        Vec3::new(15.0, 0.5, 57.5),
        0.125,
        &rink.red_net
    ));
    assert!(!is_near_net(
        Vec3::new(15.0, 0.5, 57.5),
        0.125,
        &rink.blue_net
    ));
}

#[test]
fn a_puck_that_is_not_near_a_net_does_not_touch_it() {
    let rink = Rink::new(30.0, 61.0, 8.5);
    let mut rng = Lcg(7);
    for net in [&rink.blue_net, &rink.red_net] {
        let (min, max) = (net.bounds_min - 0.5, net.bounds_max + 0.5);
        let mut near = 0;
        for _ in 0..20000 {
            let pos = rng.vec3(min, max);
            if is_near_net(pos, 0.125, net) {
                near += 1;
                continue;
            }
            assert!(
                collision_between_sphere_and_net(pos, 0.125, net).is_none(),
                "{pos:?}"
            );
            for post in &net.posts {
                assert!(
                    collision_between_sphere_and_post(pos, 0.125, post).is_none(),
                    "{pos:?}"
                );
            }
        }
        // Both sides of the test are tried
        assert!((2000..18000).contains(&near), "{near}");
    }
}

// Stick collisions

#[test]
fn a_puck_vertex_inside_the_stick_is_pushed_out_the_side_it_came_from() {
    let (overlap, normal) = collision_between_puck_vertex_and_stick(
        Vec3::new(0.1, 0.0, 0.0),
        Vec3::new(0.02, 0.0, 0.0),
        Vec3::ZERO,
        Rot3::IDENTITY,
    )
    .unwrap();
    assert!((overlap - 0.01125).abs() < 1e-6, "{overlap}");
    assert_eq!(normal, Vec3::X);
}

#[test]
fn a_puck_vertex_outside_the_stick_does_not_touch_it() {
    let collision = collision_between_puck_vertex_and_stick(
        Vec3::new(0.1, 0.0, 0.0),
        Vec3::new(0.05, 0.0, 0.0),
        Vec3::ZERO,
        Rot3::IDENTITY,
    );
    assert!(collision.is_none());
}

#[test]
fn the_stick_collision_follows_the_rotation_of_the_stick() {
    let rot = Rot3::from_rotation_y(FRAC_PI_2);
    let stick_pos = Vec3::new(3.0, 0.0, 3.0);
    let (_, normal) = collision_between_puck_vertex_and_stick(
        stick_pos + rot * Vec3::new(0.1, 0.0, 0.0),
        stick_pos + rot * Vec3::new(0.02, 0.0, 0.0),
        stick_pos,
        rot,
    )
    .unwrap();
    assert_close(normal, rot * Vec3::X, 1e-6);
}

#[test]
fn puck_vertices_are_on_the_rim_and_faces_of_the_puck() {
    let mut puck = Puck::new(
        Vec3::new(5.0, 1.0, 7.0),
        Rot3::from_rotation_x(0.4) * Rot3::from_rotation_y(1.1),
    );
    puck.body.pos = Vec3::new(5.0, 1.0, 7.0);
    let vertices = puck.get_puck_vertices();
    let mut heights = [0; 3];
    for vertex in vertices {
        let local = puck.body.rot.inverse() * (vertex - puck.body.pos);
        let rim_distance = Vec2::new(local.x, local.z).length();
        assert!((rim_distance - puck.radius).abs() < 1e-5, "{local:?}");
        let height = [-puck.height, 0.0, puck.height]
            .iter()
            .position(|h| (local.y - h).abs() < 1e-5)
            .unwrap();
        heights[height] += 1;
    }
    assert_eq!(heights, [16, 16, 16]);
    let center = vertices.iter().sum::<Vec3>() / 48.0;
    assert_close(center, puck.body.pos, 1e-5);
}

// Pucks

#[test]
fn a_puck_in_the_air_falls_with_gravity() {
    let mut sim = Sim::new();
    let puck = sim.puck(Vec3::new(15.0, 5.0, 30.5), Vec3::ZERO);
    sim.step();
    let gravity = sim.server.physics_config.gravity;
    let body = &sim.get_puck(puck).body;
    assert!((body.linear_velocity.y + gravity).abs() < 1e-8);
    // The puck moves in ten substeps, which round a little at this height
    assert!((body.pos.y - (5.0 - gravity)).abs() < 1e-5);
}

#[test]
fn a_dropped_puck_comes_to_rest_on_the_ice() {
    let mut sim = Sim::new();
    let puck = sim.puck(Vec3::new(15.0, 1.0, 30.5), Vec3::ZERO);
    sim.steps(300);
    let body = &sim.get_puck(puck).body;
    assert!(
        (body.pos.y - RESTING_PUCK_HEIGHT).abs() < 0.002,
        "{:?}",
        body.pos
    );
    assert!(body.linear_velocity.length() < 1e-3);
    assert_close(body.rot * Vec3::Y, Vec3::Y, 1e-3);
}

#[test]
fn a_sliding_puck_slows_down_in_a_straight_line() {
    let mut sim = Sim::new();
    let puck = sim.sliding_puck(15.0, 30.5, Vec3::new(0.0, 0.0, 0.2));
    let mut last_speed = 0.2;
    for _ in 0..10 {
        sim.steps(10);
        let body = &sim.get_puck(puck).body;
        let speed = body.linear_velocity.z;
        assert!(0.0 < speed && speed < last_speed, "{speed} {last_speed}");
        last_speed = speed;
        assert!((body.pos.x - 15.0).abs() < 1e-4, "{:?}", body.pos);
    }
}

#[test]
fn air_drag_grows_with_the_square_of_the_speed() {
    let mut sim = Sim::new();
    let puck = sim.puck(Vec3::new(15.0, 10.0, 30.5), Vec3::new(0.5, 0.0, 0.0));
    sim.step();
    let velocity = sim.get_puck(puck).body.linear_velocity;
    // Lose speed |v|² / 64 along the velocity
    assert!(
        (velocity.x - (0.5 - 0.5 * 0.5 / 64.0)).abs() < 1e-5,
        "{velocity:?}"
    );
}

/// How far a puck is outside the rink, or 0 if it is inside.
fn distance_outside_rink(pos: Vec3) -> f32 {
    [-pos.x, pos.x - 30.0, -pos.z, pos.z - 61.0, -pos.y]
        .into_iter()
        .fold(0.0, f32::max)
}

#[test]
fn a_puck_bounces_off_the_boards() {
    let mut sim = Sim::new();
    let puck = sim.sliding_puck(2.0, 30.5, Vec3::new(-0.2, 0.0, 0.0));
    let mut min_x = f32::MAX;
    for _ in 0..100 {
        sim.step();
        min_x = min_x.min(sim.get_puck(puck).body.pos.x);
    }
    let body = &sim.get_puck(puck).body;
    assert!(body.linear_velocity.x > 0.0, "{:?}", body.linear_velocity);
    assert!(body.pos.x > puck_radius(), "{:?}", body.pos);
    // Pucks only collide with the rink at the start of a step, so a puck can move up to one
    // step's travel into the boards before it bounces
    assert!(min_x > -0.2, "{min_x}");
}

fn puck_radius() -> f32 {
    Puck::new(Vec3::ZERO, Rot3::IDENTITY).radius
}

#[test]
fn pucks_stay_inside_the_rink() {
    let mut sim = Sim::new();
    let mut rng = Lcg(3);
    for _ in 0..30 {
        let pos = rng.vec3(Vec3::new(1.0, 0.1, 1.0), Vec3::new(29.0, 2.0, 60.0));
        let velocity = rng.vec3(Vec3::splat(-0.6), Vec3::splat(0.6));
        sim.puck(pos, velocity);
    }
    for step in 0..2000 {
        sim.step();
        for puck in sim.pucks() {
            let pos = puck.body.pos;
            assert!(pos.is_finite(), "step {step}: {pos:?}");
            // The boards and the ice push back like springs, and only the vertices of a puck
            // touch them, so a puck can sink in a bit, the most at the first fast bounces
            let limit = if step < 100 { 0.6 } else { 0.2 };
            assert!(distance_outside_rink(pos) < limit, "step {step}: {pos:?}");
        }
    }
    for puck in sim.pucks() {
        assert_eq!(
            distance_outside_rink(puck.body.pos),
            0.0,
            "{:?}",
            puck.body.pos
        );
    }
}

#[test]
fn a_spinning_puck_keeps_a_valid_rotation() {
    let mut sim = Sim::new();
    let puck = sim.sliding_puck(15.0, 30.5, Vec3::new(0.05, 0.0, 0.1));
    let mut rng = Lcg(11);
    for _ in 0..20 {
        let velocity = rng.vec3(Vec3::splat(-0.3), Vec3::splat(0.3));
        sim.get_puck_mut(puck).body.angular_velocity = velocity;
        sim.steps(50);
    }
    let rot = sim.get_puck(puck).body.rot;
    assert!((rot.length() - 1.0).abs() < 1e-4, "{}", rot.length());
}

// Pucks and nets

#[test]
fn a_puck_shot_into_the_blue_net_enters_it_and_stays_in_front_of_its_back() {
    let mut sim = Sim::new();
    let puck = sim.puck(Vec3::new(15.0, 0.3, 10.0), Vec3::new(0.0, 0.0, -0.3));
    let mut events = vec![];
    let mut min_z = f32::MAX;
    for _ in 0..300 {
        events.extend(sim.step());
        min_z = min_z.min(sim.get_puck(puck).body.pos.z);
    }
    assert!(entered_net(&events, Team::Blue));
    assert!(touched_net(&events, Team::Blue));
    assert!(!passed_goal_line(&events, Team::Blue));
    assert!(!entered_net(&events, Team::Red));
    // The back of the net is at z = 3, so the puck went past the goal line but stopped there
    assert!((3.0..4.0).contains(&min_z), "{min_z}");
}

#[test]
fn a_puck_shot_into_the_red_net_enters_it() {
    let mut sim = Sim::new();
    let puck = sim.puck(Vec3::new(15.0, 0.3, 51.0), Vec3::new(0.0, 0.0, 0.3));
    let mut events = vec![];
    let mut max_z = f32::MIN;
    for _ in 0..300 {
        events.extend(sim.step());
        max_z = max_z.max(sim.get_puck(puck).body.pos.z);
    }
    assert!(entered_net(&events, Team::Red));
    assert!(!entered_net(&events, Team::Blue));
    assert!((57.0..58.0).contains(&max_z), "{max_z}");
}

#[test]
fn a_puck_shot_wide_of_the_net_passes_the_goal_line() {
    let mut sim = Sim::new();
    sim.sliding_puck(11.0, 10.0, Vec3::new(0.0, 0.0, -0.3));
    let events = sim.steps(300);
    assert!(passed_goal_line(&events, Team::Blue));
    assert!(!entered_net(&events, Team::Blue));
    assert!(!touched_net(&events, Team::Blue));
}

#[test]
fn a_puck_shot_at_a_post_bounces_back_out() {
    let mut sim = Sim::new();
    // Straight at the right post of the blue net, at x = 16.5
    let puck = sim.puck(Vec3::new(16.5, 0.3, 10.0), Vec3::new(0.0, 0.0, -0.3));
    let events = sim.steps(300);
    assert!(touched_net(&events, Team::Blue));
    assert!(!entered_net(&events, Team::Blue));
    // The post takes most of the speed, but the puck rolls back out
    let body = &sim.get_puck(puck).body;
    assert!(body.pos.z > 5.0, "{:?}", body.pos);
    assert!((body.pos.x - 16.5).abs() < 1e-4, "{:?}", body.pos);
}

#[test]
fn a_puck_shot_at_the_back_of_the_net_stays_out() {
    let mut sim = Sim::new();
    let puck = sim.puck(Vec3::new(15.0, 0.3, 0.5), Vec3::new(0.0, 0.0, 0.2));
    let mut events = vec![];
    let mut max_z = f32::MIN;
    for _ in 0..300 {
        events.extend(sim.step());
        max_z = max_z.max(sim.get_puck(puck).body.pos.z);
    }
    assert!(touched_net(&events, Team::Blue));
    assert!(!entered_net(&events, Team::Blue));
    assert!(!passed_goal_line(&events, Team::Blue));
    assert!(max_z < 3.25, "{max_z}");
}

#[test]
fn a_puck_dropped_on_the_roof_of_the_net_stays_on_top() {
    let mut sim = Sim::new();
    let puck = sim.puck(Vec3::new(15.0, 2.0, 3.6), Vec3::ZERO);
    let events = sim.steps(300);
    let pos = sim.get_puck(puck).body.pos;
    assert!(touched_net(&events, Team::Blue));
    assert!(!entered_net(&events, Team::Blue));
    assert!(pos.y > 1.0, "{pos:?}");
}

#[test]
fn shots_at_the_two_nets_behave_the_same_way() {
    let mut rng = Lcg(5);
    for _ in 0..10 {
        let start = rng.vec3(Vec3::new(11.0, 0.05, 6.0), Vec3::new(19.0, 1.0, 12.0));
        let target = rng.vec3(Vec3::new(13.0, 0.0, 3.0), Vec3::new(17.0, 1.0, 4.0));
        let velocity = (target - start).normalize() * rng.range(0.1, 0.4);

        let mut blue = Sim::new();
        let blue_puck = blue.puck(start, velocity);
        let mut red = Sim::new();
        let red_puck = red.puck(
            turned(start),
            Vec3::new(-velocity.x, velocity.y, -velocity.z),
        );

        // The coordinates round differently at the two ends, so the paths drift apart a little
        // after bounces, and a touch of the net at the edge of a substep can be counted at one
        // end but not the other. The same kinds of events happen, though.
        let mut blue_events = BTreeSet::new();
        let mut red_events = BTreeSet::new();
        for _ in 0..200 {
            for event in blue.step() {
                blue_events.insert(format!("{:?}", with_teams_swapped(event)));
            }
            for event in red.step() {
                red_events.insert(format!("{event:?}"));
            }
            let blue_pos = blue.get_puck(blue_puck).body.pos;
            let red_pos = red.get_puck(red_puck).body.pos;
            assert_close(turned(blue_pos), red_pos, 0.1);
        }
        assert_eq!(blue_events, red_events, "from {start:?}");
        assert!(!red_events.is_empty());
    }
}

// Lines

fn line_events(events: &[PhysicsEvent]) -> Vec<String> {
    events
        .iter()
        .filter(|event| {
            !matches!(
                event,
                PhysicsEvent::PuckTouch { .. }
                    | PhysicsEvent::PuckTouchedNet { .. }
                    | PhysicsEvent::PuckEnteredNet { .. }
                    | PhysicsEvent::PuckPassedGoalLine { .. }
            )
        })
        .map(|event| {
            let text = format!("{event:?}");
            text.split(", puck").next().unwrap().to_owned() + " }"
        })
        .collect()
}

#[test]
fn a_puck_from_the_red_end_crosses_all_lines_in_order() {
    let mut sim = Sim::new();
    sim.sliding_puck(15.0, 45.0, Vec3::new(0.0, 0.0, -0.4));
    let events = sim.steps(300);
    assert_eq!(
        line_events(&events),
        [
            "PuckReachedDefensiveLine { team: Red }",
            "PuckPassedDefensiveLine { team: Red }",
            "PuckReachedCenterLine { team: Red }",
            "PuckPassedCenterLine { team: Red }",
            "PuckReachedOffensiveZone { team: Red }",
            "PuckEnteredOffensiveZone { team: Red }",
        ]
    );
}

#[test]
fn a_puck_from_the_blue_end_crosses_all_lines_in_order() {
    let mut sim = Sim::new();
    sim.sliding_puck(15.0, 16.0, Vec3::new(0.0, 0.0, 0.4));
    let events = sim.steps(300);
    assert_eq!(
        line_events(&events),
        [
            "PuckReachedDefensiveLine { team: Blue }",
            "PuckPassedDefensiveLine { team: Blue }",
            "PuckReachedCenterLine { team: Blue }",
            "PuckPassedCenterLine { team: Blue }",
            "PuckReachedOffensiveZone { team: Blue }",
            "PuckEnteredOffensiveZone { team: Blue }",
        ]
    );
}

#[test]
fn a_puck_that_stops_on_a_line_has_reached_but_not_passed_it() {
    let mut sim = Sim::new();
    // The puck touches the center line, at z = 30.5, when it is within 0.275 of it. This puck
    // slides about 0.06 before it stops.
    let puck = sim.sliding_puck(15.0, 30.785, Vec3::new(0.0, 0.0, -0.002));
    let events = sim.steps(200);
    assert_eq!(
        line_events(&events),
        ["PuckReachedCenterLine { team: Red }"]
    );
    let z = sim.get_puck(puck).body.pos.z;
    assert!((30.5..30.775).contains(&z), "{z}");
}

// Skaters

#[test]
fn a_skater_settles_upright_on_the_ice() {
    let mut sim = Sim::new();
    let player = sim.skater(Team::Red, Vec3::new(15.0, 1.5, 30.5), Rot3::IDENTITY);
    sim.steps(300);
    let skater = sim.get_skater(player);
    assert!(
        (0.4..0.8).contains(&skater.body.pos.y),
        "{:?}",
        skater.body.pos
    );
    assert!((skater.body.rot * Vec3::Y).y > 0.99);
    assert!(skater.body.linear_velocity.length() < 0.005);
}

#[test]
fn skating_forward_reaches_but_does_not_pass_the_top_speed() {
    let mut sim = Sim::new();
    let player = sim.settled_skater(Vec3::new(15.0, 1.5, 50.0), Rot3::IDENTITY);
    sim.input(player).fwbw = 1.0;
    let max_speed = sim.server.physics_config.max_player_speed;
    for _ in 0..400 {
        sim.step();
        let speed = sim.get_skater(player).body.linear_velocity.length();
        assert!(speed <= max_speed * 1.01, "{speed}");
    }
    let skater = sim.get_skater(player);
    let velocity = skater.body.linear_velocity;
    assert!(velocity.length() > max_speed * 0.95, "{velocity:?}");
    assert!(velocity.normalize().dot(facing(skater)) > 0.99);
}

#[test]
fn skating_backward_moves_the_other_way() {
    let mut sim = Sim::new();
    let player = sim.settled_skater(Vec3::new(15.0, 1.5, 30.5), Rot3::IDENTITY);
    sim.input(player).fwbw = -1.0;
    sim.steps(200);
    let skater = sim.get_skater(player);
    assert!(skater.body.linear_velocity.normalize().dot(facing(skater)) < -0.99);
}

#[test]
fn turning_turns_the_skater_to_either_side() {
    for (turn, side) in [(1.0, 1.0), (-1.0, -1.0)] {
        let mut sim = Sim::new();
        let player = sim.settled_skater(Vec3::new(15.0, 1.5, 30.5), Rot3::IDENTITY);
        sim.input(player).turn = turn;
        sim.steps(100);
        let facing = facing(sim.get_skater(player));
        // Facing -Z to start with, turning right turns towards +X
        assert!(facing.x * side > 0.3, "turn {turn}: {facing:?}");
    }
}

#[test]
fn shift_turning_moves_the_skater_sideways() {
    let mut sim = Sim::new();
    let player = sim.settled_skater(Vec3::new(15.0, 1.5, 30.5), Rot3::IDENTITY);
    let input = sim.input(player);
    input.keys = SHIFT_KEY;
    input.turn = 1.0;
    sim.steps(100);
    let skater = sim.get_skater(player);
    let sideways = skater.body.rot * Vec3::X;
    let velocity = skater.body.linear_velocity;
    assert!(velocity.dot(sideways) > 0.01, "{velocity:?}");
    assert!(velocity.dot(sideways) > velocity.dot(facing(skater)).abs());
}

#[test]
fn a_skater_is_stopped_by_the_boards() {
    let mut sim = Sim::new();
    let player = sim.settled_skater(Vec3::new(15.0, 1.5, 5.0), Rot3::IDENTITY);
    sim.input(player).fwbw = 1.0;
    for step in 0..600 {
        sim.step();
        for ball in &sim.get_skater(player).collision_balls {
            assert!(ball.pos.z > -0.1, "step {step}: {:?}", ball.pos);
        }
    }
    // The stick reaches the boards first, so the skater stops a little away from them
    let skater = sim.get_skater(player);
    assert!(skater.body.pos.z < 2.0, "{:?}", skater.body.pos);
    assert!(skater.stick_pos.z > 0.0, "{:?}", skater.stick_pos);
}

#[test]
fn skaters_skating_into_each_other_do_not_pass_through_each_other() {
    let mut sim = Sim::new();
    let a = sim.skater(Team::Red, Vec3::new(15.0, 1.5, 35.0), Rot3::IDENTITY);
    let b = sim.skater(Team::Blue, Vec3::new(15.1, 1.5, 26.0), HALF_TURN_Y);
    sim.steps(200);
    sim.input(a).fwbw = 1.0;
    sim.input(b).fwbw = 1.0;
    let mut min_distance = f32::MAX;
    for _ in 0..400 {
        sim.step();
        let a_pos = sim.get_skater(a).body.pos;
        let b_pos = sim.get_skater(b).body.pos;
        min_distance = min_distance.min(Vec2::new(a_pos.x - b_pos.x, a_pos.z - b_pos.z).length());
    }
    assert!(min_distance > 0.3, "{min_distance}");
}

#[test]
fn skaters_bump_each_other_apart() {
    let mut sim = Sim::new();
    let a = sim.skater(Team::Red, Vec3::new(15.0, 1.5, 30.5), Rot3::IDENTITY);
    let b = sim.skater(Team::Blue, Vec3::new(15.0, 1.5, 30.2), Rot3::IDENTITY);
    sim.steps(100);
    let distance = sim
        .get_skater(a)
        .body
        .pos
        .distance(sim.get_skater(b).body.pos);
    assert!(distance > 0.4, "{distance}");
}

#[test]
fn a_skater_jumps_once_per_key_press() {
    let mut sim = Sim::new();
    let player = sim.settled_skater(Vec3::new(15.0, 1.5, 30.5), Rot3::IDENTITY);
    let rest_y = sim.get_skater(player).body.pos.y;
    sim.input(player).keys = JUMP_KEY;
    let mut jumps = 0;
    let mut max_y = rest_y;
    for _ in 0..150 {
        let before = sim.get_skater(player).body.linear_velocity.y;
        sim.step();
        let skater = sim.get_skater(player);
        if skater.body.linear_velocity.y - before > 0.02 {
            jumps += 1;
        }
        max_y = max_y.max(skater.body.pos.y);
    }
    assert_eq!(jumps, 1);
    assert!(max_y > rest_y + 0.2, "{rest_y} {max_y}");

    // Releasing the key and pressing it again jumps again once the skater is on the ice
    sim.input(player).keys = 0;
    sim.step();
    sim.input(player).keys = JUMP_KEY;
    let before = sim.get_skater(player).body.linear_velocity.y;
    sim.step();
    assert!(sim.get_skater(player).body.linear_velocity.y - before > 0.02);
}

#[test]
fn crouching_lowers_the_skater_and_standing_up_raises_it_again() {
    let mut sim = Sim::new();
    let player = sim.settled_skater(Vec3::new(15.0, 1.5, 30.5), Rot3::IDENTITY);
    let standing_y = sim.get_skater(player).body.pos.y;
    sim.input(player).keys = CROUCH_KEY;
    sim.steps(100);
    let skater = sim.get_skater(player);
    assert_eq!(skater.height, 0.25);
    // A skater who is low and slow is pushed up a little, so it does not sink all the way
    assert!(
        skater.body.pos.y < standing_y - 0.05,
        "{:?}",
        skater.body.pos
    );

    sim.input(player).keys = 0;
    sim.steps(4);
    assert_eq!(sim.get_skater(player).height, 0.75);
}

// Sticks

#[test]
fn the_stick_placement_follows_the_input_within_its_limits() {
    let mut sim = Sim::new();
    let player = sim.settled_skater(Vec3::new(15.0, 1.5, 30.5), Rot3::IDENTITY);
    sim.input(player).stick = Vec2::new(5.0, 5.0);
    sim.steps(300);
    let placement = sim.get_skater(player).stick_placement;
    assert!(
        placement.abs_diff_eq(Vec2::new(FRAC_PI_2, FRAC_PI_8), 1e-3),
        "{placement:?}"
    );

    sim.input(player).stick = Vec2::new(-5.0, -5.0);
    sim.steps(300);
    let placement = sim.get_skater(player).stick_placement;
    assert!(
        placement.abs_diff_eq(Vec2::new(-FRAC_PI_2, -5.0 * PI / 16.0), 1e-3),
        "{placement:?}"
    );
}

#[test]
fn the_stick_moves_a_limited_amount_per_step() {
    let mut sim = Sim::new();
    let player = sim.settled_skater(Vec3::new(15.0, 1.5, 30.5), Rot3::IDENTITY);
    sim.input(player).stick = Vec2::new(FRAC_PI_2, 0.0);
    let mut last = sim.get_skater(player).stick_placement_delta;
    for _ in 0..50 {
        sim.step();
        let delta = sim.get_skater(player).stick_placement_delta;
        assert!((delta - last).length() <= 0.008_888_889 + 1e-7);
        last = delta;
    }
}

#[test]
fn the_stick_does_not_go_through_the_ice() {
    let mut sim = Sim::new();
    let player = sim.settled_skater(Vec3::new(15.0, 1.5, 30.5), Rot3::IDENTITY);
    let input = sim.input(player);
    input.stick = Vec2::new(0.0, -5.0 * PI / 16.0);
    input.body_rot = FRAC_PI_2;
    input.keys = CROUCH_KEY;
    for step in 0..300 {
        sim.step();
        let stick_pos = sim.get_skater(player).stick_pos;
        assert!(stick_pos.y > -0.05, "step {step}: {stick_pos:?}");
    }
}

#[test]
fn left_and_right_handed_skaters_hold_the_stick_on_opposite_sides() {
    let mut sides = vec![];
    for hand in [SkaterHand::Right, SkaterHand::Left] {
        let mut sim = Sim::new();
        let player = sim.skater(Team::Red, Vec3::new(15.0, 1.5, 30.5), Rot3::IDENTITY);
        sim.get_skater_mut(player).hand = hand;
        sim.steps(200);
        let skater = sim.get_skater(player);
        let local = skater.body.rot.inverse() * (skater.stick_pos - skater.body.pos);
        sides.push(local.x);
    }
    assert!(sides[0] < -0.1 && sides[1] > 0.1, "{sides:?}");
}

#[test]
fn a_stick_swinging_through_a_puck_touches_and_moves_it() {
    let mut sim = Sim::new();
    let player = sim.settled_skater(Vec3::new(15.0, 1.5, 30.5), Rot3::IDENTITY);
    // With the stick straight ahead, the blade rests on the ice where the puck will be put
    let puck_pos = sim.get_skater(player).stick_pos;
    sim.input(player).stick = Vec2::new(-1.0, 0.0);
    sim.steps(100);
    let puck = sim.sliding_puck(puck_pos.x, puck_pos.z, Vec3::ZERO);
    sim.input(player).stick = Vec2::new(1.0, 0.0);
    let events = sim.steps(100);
    assert!(events.iter().any(|event| matches!(
        event,
        PhysicsEvent::PuckTouch { player: p, puck: q } if *p == player && *q == puck
    )));
    let moved = sim.get_puck(puck).body.pos - puck_pos;
    assert!(Vec2::new(moved.x, moved.z).length() > 0.3, "{moved:?}");
}

#[test]
fn a_stick_away_from_the_puck_does_not_touch_it() {
    let mut sim = Sim::new();
    sim.settled_skater(Vec3::new(15.0, 1.5, 30.5), Rot3::IDENTITY);
    sim.sliding_puck(20.0, 30.5, Vec3::ZERO);
    let events = sim.steps(100);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, PhysicsEvent::PuckTouch { .. }))
    );
}

// Inputs and determinism

/// Runs a skater and a puck with the given input for a while, and returns the resulting state.
fn run_with_input(input: PlayerInput) -> String {
    let mut sim = Sim::new();
    let player = sim.settled_skater(Vec3::new(15.0, 1.5, 30.5), Rot3::IDENTITY);
    sim.sliding_puck(15.3, 29.0, Vec3::ZERO);
    *sim.input(player) = input;
    sim.steps(200);
    sim.snapshot()
}

fn busy_input() -> PlayerInput {
    PlayerInput {
        stick_angle: 0.5,
        turn: 0.7,
        fwbw: 1.0,
        stick: Vec2::new(0.8, -0.4),
        head_rot: 1.0,
        body_rot: -0.5,
        keys: CROUCH_KEY,
    }
}

#[test]
fn the_simulation_is_deterministic() {
    assert_eq!(run_with_input(busy_input()), run_with_input(busy_input()));
}

#[test]
fn nan_inputs_count_as_no_input() {
    let nan = PlayerInput {
        stick_angle: f32::NAN,
        turn: f32::NAN,
        fwbw: f32::NAN,
        stick: Vec2::NAN,
        head_rot: f32::NAN,
        body_rot: f32::NAN,
        keys: 0,
    };
    let state = run_with_input(nan);
    assert!(!state.contains("NaN"), "{state}");
    assert_eq!(state, run_with_input(PlayerInput::default()));
}

#[test]
fn inputs_outside_their_range_are_limited_to_it() {
    let huge = PlayerInput {
        stick_angle: f32::INFINITY,
        turn: f32::INFINITY,
        fwbw: f32::INFINITY,
        stick: Vec2::new(f32::INFINITY, f32::NEG_INFINITY),
        head_rot: f32::INFINITY,
        body_rot: f32::NEG_INFINITY,
        keys: 0,
    };
    let limited = PlayerInput {
        stick_angle: 1.0,
        turn: 1.0,
        fwbw: 1.0,
        stick: Vec2::new(FRAC_PI_2, -5.0 * PI / 16.0),
        head_rot: 7.0 * FRAC_PI_8,
        body_rot: -FRAC_PI_2,
        keys: 0,
    };
    assert_eq!(run_with_input(huge), run_with_input(limited));
}

use super::*;
use crate::geom::{Grid, GroupGeom};

fn identity_group() -> AnimGroupState {
    AnimGroupState {
        pos: Vec3::ZERO,
        prev_pos: Vec3::ZERO,
        rot: [0; 3],
        prev_rot: [0; 3],
        transform: MTX_IDENTITY,
        prev_transform: MTX_IDENTITY,
    }
}

/// One 4x4 floor triangle facing +Y with corners (0,0,0), (4,0,0), (0,0,-4).
fn floor_group() -> GroupGeom {
    let tri = Tri {
        pos: Vec3::ZERO,
        normal: Vec3::new(0.0, 1.0, 0.0),
        rot: [-0x4000, 0, 0],
        vert2: [4.0, 0.0],
        vert3: [0.0, 4.0],
        edge2_normal: [
            -std::f64::consts::FRAC_1_SQRT_2,
            -std::f64::consts::FRAC_1_SQRT_2,
        ],
        edge3_normal: [1.0, 0.0],
    };
    GroupGeom {
        init_pos: Vec3::ZERO,
        init_rot: [0; 3],
        unk_b8: Vec3::ZERO,
        anim: None,
        triangles: vec![tri],
        grid: Grid {
            origin_x: -2.0,
            origin_z: -6.0,
            step_x: 8.0,
            step_z: 8.0,
            count_x: 1,
            count_z: 1,
            cells: vec![vec![0]],
        },
        cones: vec![],
        spheres: vec![],
        cylinders: vec![],
        goals: vec![],
        bumpers: vec![],
        jamabars: vec![],
        bananas: vec![],
        fallout_boxes: vec![],
    }
}

fn geom() -> StageGeom {
    StageGeom {
        stage_id: 1,
        loop_start_seconds: 0.0,
        loop_end_seconds: 0.0,
        groups: vec![floor_group()],
        start_positions: vec![],
        fallout_y: -10.0,
    }
}

fn ball_at(pos: Vec3, vel: Vec3) -> PhysBall {
    let mut b = PhysBall::new();
    b.prev_pos = Vec3::new(pos.x - vel.x, pos.y - vel.y, pos.z - vel.z);
    b.pos = pos;
    b.vel = vel;
    b
}

#[test]
fn plane_pushes_ball_out_and_bounces() {
    let plane = Plane {
        point: Vec3::ZERO,
        normal: Vec3::new(0.0, 1.0, 0.0),
    };
    let mut ball = ball_at(Vec3::new(0.0, 0.3, 0.0), Vec3::new(0.1, -0.3, 0.0));
    collide_ball_with_plane(&mut ball, &plane);
    assert!((ball.pos.y - 0.5).abs() < 1e-12, "pushed to the surface");
    assert!(ball.vel.y > 0.0, "fast impact bounces");
    assert!(ball.vel.x < 0.1, "friction slows the parallel velocity");
    assert_eq!(ball.flags & COLI_OCCURRED, COLI_OCCURRED);
    assert_eq!(ball.hardest_coli_plane.normal, plane.normal);
    assert!(ball.hardest_coli_speed < 0.0);

    // slow approach is absorbed instead of bounced
    let mut slow = ball_at(Vec3::new(0.0, 0.45, 0.0), Vec3::new(0.0, -0.01, 0.0));
    collide_ball_with_plane(&mut slow, &plane);
    assert!(slow.vel.y.abs() < 1e-12);

    // too far above: no contact
    let mut far = ball_at(Vec3::new(0.0, 2.0, 0.0), Vec3::new(0.0, -0.1, 0.0));
    collide_ball_with_plane(&mut far, &plane);
    assert_eq!(far.flags, 0);
    assert_eq!(far.pos.y, 2.0);
}

#[test]
fn moving_sphere_tests() {
    let a0 = Vec3::new(-2.0, 0.0, 0.0);
    let a1 = Vec3::new(2.0, 0.0, 0.0);
    let b = Vec3::ZERO;
    assert!(intersects_moving_spheres(&a0, &a1, &b, &b, 0.5, 0.5));
    let off = Vec3::new(0.0, 3.0, 0.0);
    assert!(!intersects_moving_spheres(&a0, &a1, &off, &off, 0.5, 0.5));
    // stationary pair: the reference reports no intersection (zero relative motion)
    assert!(!intersects_moving_spheres(&b, &b, &b, &b, 0.5, 0.5));
    // moving away
    let away0 = Vec3::new(1.0, 0.0, 0.0);
    let away1 = Vec3::new(5.0, 0.0, 0.0);
    assert!(!intersects_moving_spheres(&away0, &away1, &b, &b, 0.4, 0.4));

    let mut end_a = a1;
    let mut end_b = b;
    assert!(resolve_moving_spheres(
        &a0, &mut end_a, &b, &mut end_b, 0.5, 0.5
    ));
    assert!((end_a.x - -1.0).abs() < 1e-6, "stops at contact: {end_a:?}");
}

#[test]
fn ball_lands_on_triangle_and_hits_its_edge() {
    let g = geom();
    let groups = vec![identity_group()];
    let mut c = Collider::new();

    let mut ball = ball_at(Vec3::new(1.0, 0.45, -1.0), Vec3::new(0.0, -0.2, 0.0));
    c.collide_ball_with_stage(&mut ball, &g, &groups);
    assert_eq!(ball.flags & COLI_OCCURRED, COLI_OCCURRED);
    assert!((ball.pos.y - 0.5).abs() < 1e-9, "{:?}", ball.pos);
    assert!(ball.vel.y >= 0.0);

    // outside the triangle (past the hypotenuse): no collision
    let mut off = ball_at(Vec3::new(3.9, 0.6, -3.9), Vec3::new(0.0, -0.2, 0.0));
    c.collide_ball_with_stage(&mut off, &g, &groups);
    assert_eq!(off.flags & COLI_OCCURRED, 0);

    // a ball that started below the surface is ignored (one-sided)
    let mut under = ball_at(Vec3::new(1.0, -0.2, -1.0), Vec3::new(0.0, 0.2, 0.0));
    c.collide_ball_with_stage(&mut under, &g, &groups);
    assert_eq!(under.flags & COLI_OCCURRED, 0);

    // grazing the x=0 edge from outside at floor height hits the edge
    let mut edge = ball_at(Vec3::new(-0.3, 0.1, -1.0), Vec3::new(0.05, -0.02, 0.0));
    c.collide_ball_with_stage(&mut edge, &g, &groups);
    assert_eq!(edge.flags & COLI_OCCURRED, COLI_OCCURRED, "edge contact");
}

#[test]
fn raycast_finds_the_floor_below() {
    let g = geom();
    let groups = vec![identity_group()];
    let mut c = Collider::new();
    let hit = c
        .raycast_stage_down(&Vec3::new(1.0, 5.0, -1.0), &g, &groups)
        .expect("hit");
    assert!(hit.pos.y.abs() < 1e-9);
    assert_eq!(hit.normal, Vec3::new(0.0, 1.0, 0.0));
    assert!(
        c.raycast_stage_down(&Vec3::new(3.9, 5.0, -3.9), &g, &groups)
            .is_none()
    );
    assert!(
        c.raycast_stage_down(&Vec3::new(1.0, -1.0, -1.0), &g, &groups)
            .is_none()
    );
}

#[test]
fn grid_lookup_bounds() {
    let g = floor_group().grid;
    assert!(g.lookup(0.0, -1.0).is_some());
    assert!(g.lookup(-3.0, -1.0).is_none());
    assert!(g.lookup(0.0, 3.0).is_none());
    assert!(g.lookup(f64::NAN, 0.0).is_none());
    assert!(Grid::default().lookup(0.0, 0.0).is_none());
}

#[test]
fn sphere_cylinder_and_cone() {
    let groups = vec![identity_group()];
    let mut c = Collider::new();
    let mut g = geom();
    g.groups[0].triangles.clear();
    g.groups[0].grid = Grid::default();
    g.groups[0].spheres.push(Sphere {
        pos: Vec3::ZERO,
        radius: 1.0,
    });
    let mut ball = ball_at(Vec3::new(0.0, 1.3, 0.0), Vec3::new(0.0, -0.1, 0.0));
    c.collide_ball_with_stage(&mut ball, &g, &groups);
    assert!(ball.flags & COLI_OCCURRED != 0);
    assert!((ball.pos.y - 1.5).abs() < 1e-6);
    assert!(ball.hardest_coli_plane.normal.y > 0.99);

    g.groups[0].spheres.clear();
    g.groups[0].cylinders.push(Cylinder {
        pos: Vec3::ZERO,
        radius: 1.0,
        height: 2.0,
        rot: [0; 3],
    });
    // side hit: approaching along -x towards the cylinder wall
    let mut ball = ball_at(Vec3::new(1.3, 0.0, 0.0), Vec3::new(-0.1, 0.0, 0.0));
    c.collide_ball_with_stage(&mut ball, &g, &groups);
    assert!(ball.flags & COLI_OCCURRED != 0);
    assert!(ball.pos.x >= 1.49, "{:?}", ball.pos);
    // top cap: previous position above the top
    let mut ball = ball_at(Vec3::new(0.2, 1.4, 0.0), Vec3::new(0.0, -0.1, 0.0));
    c.collide_ball_with_stage(&mut ball, &g, &groups);
    assert!(ball.flags & COLI_OCCURRED != 0);
    assert!((ball.pos.y - 1.5).abs() < 1e-6, "{:?}", ball.pos);

    g.groups[0].cylinders.clear();
    g.groups[0].cones.push(Cone {
        pos: Vec3::ZERO,
        rot: [0; 3],
        scale: Vec3::new(1.0, 2.0, 1.0),
    });
    let mut ball = ball_at(Vec3::new(1.0, 0.5, 0.0), Vec3::new(-0.05, -0.05, 0.0));
    c.collide_ball_with_stage(&mut ball, &g, &groups);
    assert!(ball.flags & COLI_OCCURRED != 0);
}

#[test]
fn trigger_rect_crossing() {
    let mut c = Collider::new();
    let rect = TriggerRect {
        pos: Vec3::new(0.0, 1.0, 0.0),
        rot: [0; 3],
        width: 2.0,
        height: 2.0,
    };
    // crosses the local z=0 plane inside the rect
    assert!(c.test_line_intersects_rect(
        &Vec3::new(0.2, 1.0, 0.5),
        &Vec3::new(0.2, 1.0, -0.5),
        &rect
    ));
    // same side
    assert!(!c.test_line_intersects_rect(
        &Vec3::new(0.2, 1.0, 0.5),
        &Vec3::new(0.2, 1.0, 0.1),
        &rect
    ));
    // crosses outside the rect
    assert!(!c.test_line_intersects_rect(
        &Vec3::new(3.0, 1.0, 0.5),
        &Vec3::new(3.0, 1.0, -0.5),
        &rect
    ));
}

#[test]
fn bonus_wave_footprint() {
    assert!(bonus_wave_surface_at(11.0, 0.0, 100.0).is_none());
    assert!(bonus_wave_surface_at(0.0, -11.0, 100.0).is_none());
    let flat = bonus_wave_surface_at(5.0, 5.0, 0.0).unwrap();
    assert_eq!(flat.normal, Vec3::new(0.0, 1.0, 0.0));
    assert_eq!(flat.point.y, 0.0);
    let wave = bonus_wave_surface_at(2.0, 1.0, 1000.0).unwrap();
    assert!(wave.normal.y > 0.0);
}

#[test]
fn anim_group_space_roundtrip() {
    let mut moving = identity_group();
    let mut s = MatrixStack::new();
    s.from_translate(&Vec3::new(5.0, 0.0, 0.0));
    s.rotate_y(0x2000);
    moving.transform = s.a;
    moving.prev_transform = s.a;
    let groups = vec![identity_group(), moving];
    let mut c = Collider::new();
    let mut ball = ball_at(Vec3::new(6.0, 1.0, 2.0), Vec3::new(0.1, 0.0, 0.0));
    let start = ball.clone();
    c.tf_physball_to_group(&mut ball, 1, &groups);
    assert_eq!(ball.anim_group_id, 1);
    assert!(
        (ball.pos.x - start.pos.x).abs() > 0.1,
        "moved into group space"
    );
    c.tf_physball_to_group(&mut ball, 0, &groups);
    assert_eq!(ball.anim_group_id, 0);
    assert!((ball.pos.x - start.pos.x).abs() < 1e-5);
    assert!((ball.pos.z - start.pos.z).abs() < 1e-5);
    assert!((ball.vel.x - start.vel.x).abs() < 1e-5);
}

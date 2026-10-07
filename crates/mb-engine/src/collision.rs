//! Port of WebMonkeyBall `src/collision.ts` (SMB1 paths): ball vs triangle faces/edges/vertices, cones, spheres,
//! cylinders, goal rings, bumpers, jamabars, goal bag/tape, raycast, anim-group space transforms.
//!
//! Order of operations is kept identical to the reference, including its quirks, so results are bit-exact.

use crate::constants::COLI_OCCURRED;
use crate::geom::{Cone, Cylinder, GoalGeom, GroupGeom, Sphere, StageGeom, Tri};
use crate::math::*;
use crate::objects::*;

const GOAL_RING_MIN_DOT: f64 = 0.7732404444;
const GOAL_RING_RADIUS: f64 = 2.25;
const GOAL_RING_THICKNESS: f64 = 0.1;
const GOAL_POST_RADIUS: f64 = 0.2;
const GOAL_POST_HEIGHT: f64 = 1.5;
const BONUS_WAVE_STAGE_ID: u32 = 92;
const BONUS_WAVE_LIMIT: f64 = 10.01;
const BONUS_WAVE_AMPLITUDE_BASE: f64 = 0.5;
const BONUS_WAVE_AMPLITUDE_SLOPE: f64 = -0.030833333333333333;
const BONUS_WAVE_ANGLE_SPEED: f64 = -1092.0;
const BONUS_WAVE_START_FRAME: f64 = 30.0;
const BONUS_WAVE_ANGLE_SCALE: f64 = 16384.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plane {
    pub point: Vec3,
    pub normal: Vec3,
}

/// The reference's `physBall`: the ball as seen by the collision code, in the space of `anim_group_id`.
#[derive(Debug, Clone, PartialEq)]
pub struct PhysBall {
    pub flags: u32,
    pub pos: Vec3,
    pub prev_pos: Vec3,
    pub vel: Vec3,
    pub radius: f64,
    pub gravity_accel: f64,
    pub restitution: f64,
    pub hardest_coli_speed: f64,
    pub hardest_coli_plane: Plane,
    pub hardest_coli_anim_group_id: usize,
    pub friction: f64,
    pub anim_group_id: usize,
}

impl PhysBall {
    pub fn new() -> PhysBall {
        PhysBall {
            flags: 0,
            pos: Vec3::ZERO,
            prev_pos: Vec3::ZERO,
            vel: Vec3::ZERO,
            radius: 0.5,
            gravity_accel: 0.009799992,
            restitution: 0.5,
            hardest_coli_speed: 0.0,
            hardest_coli_plane: Plane {
                point: Vec3::ZERO,
                normal: Vec3::new(0.0, 1.0, 0.0),
            },
            hardest_coli_anim_group_id: 0,
            friction: 0.01,
            anim_group_id: 0,
        }
    }
}

impl Default for PhysBall {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy)]
struct Edge {
    start: [f64; 2],
    end: [f64; 2],
    normal: [f64; 2],
}

struct Rect {
    pos: Vec3,
    normal: Vec3,
    width: f64,
    height: f64,
}

const JAMABAR_RECTS: [Rect; 4] = [
    Rect {
        pos: Vec3::new(0.0, 0.5, 1.75),
        normal: Vec3::new(0.0, 0.0, 1.0),
        width: 1.0,
        height: 1.0,
    },
    Rect {
        pos: Vec3::new(-0.5, 0.5, 0.0),
        normal: Vec3::new(-1.0, 0.0, 0.0),
        width: 3.5,
        height: 1.0,
    },
    Rect {
        pos: Vec3::new(0.0, 0.5, -1.75),
        normal: Vec3::new(0.0, 0.0, -1.0),
        width: 1.0,
        height: 1.0,
    },
    Rect {
        pos: Vec3::new(0.5, 0.5, 0.0),
        normal: Vec3::new(1.0, 0.0, 0.0),
        width: 3.5,
        height: 1.0,
    },
];

/// Trigger rectangle for goal/wormhole tests.
pub struct TriggerRect {
    pub pos: Vec3,
    pub rot: [i32; 3],
    pub width: f64,
    pub height: f64,
}

/// Result of [`Collider::raycast_stage_down`].
#[derive(Debug, Clone, PartialEq)]
pub struct RayHit {
    pub pos: Vec3,
    pub normal: Vec3,
    pub surface_vel: Vec3,
    pub flags: u32,
}

fn dumb_dot(x1: f64, y1: f64, x2: f64, y2: f64) -> f64 {
    x1 * x2 + y1 * y2
}

pub fn intersects_moving_spheres(
    start_a: &Vec3,
    end_a: &Vec3,
    start_b: &Vec3,
    end_b: &Vec3,
    radius_a: f64,
    radius_b: f64,
) -> bool {
    let dx = start_a.x - start_b.x;
    let dy = start_a.y - start_b.y;
    let dz = start_a.z - start_b.z;
    let vx = (end_a.x - end_b.x) - dx;
    let vy = (end_a.y - end_b.y) - dy;
    let vz = (end_a.z - end_b.z) - dz;
    let vv = (vx * vx) + (vy * vy) + (vz * vz);
    if vv == 0.0 {
        return false;
    }
    let radius_sum = radius_a + radius_b;
    let vd = (dx * vx) + (dy * vy) + (dz * vz);
    let dd = (dx * dx) + (dy * dy) + (dz * dz) - (radius_sum * radius_sum);
    if (vd * vd) - (vv * dd) < 0.0 {
        return false;
    }
    if dd <= 0.0 {
        return true;
    }
    if (vv + vd + vd + dd) <= 0.0 {
        return true;
    }
    if vd >= 0.0 {
        return false;
    }
    if -vd >= vv {
        return false;
    }
    true
}

fn resolve_moving_spheres(
    start_a: &Vec3,
    end_a: &mut Vec3,
    start_b: &Vec3,
    end_b: &mut Vec3,
    radius_a: f64,
    radius_b: f64,
) -> bool {
    let dx = start_a.x - start_b.x;
    let dy = start_a.y - start_b.y;
    let dz = start_a.z - start_b.z;
    let vx = (end_a.x - end_b.x) - dx;
    let vy = (end_a.y - end_b.y) - dy;
    let vz = (end_a.z - end_b.z) - dz;
    let vv = (vx * vx) + (vy * vy) + (vz * vz);
    if vv == 0.0 {
        return false;
    }
    let radius_sum = radius_a + radius_b;
    let vd = (dx * vx) + (dy * vy) + (dz * vz);
    let dd = (dx * dx) + (dy * dy) + (dz * dz) - (radius_sum * radius_sum);
    let discriminant = (vd * vd) - (vv * dd);
    if discriminant < 0.0 {
        return false;
    }
    if dd <= 0.0 {
        *end_a = *start_a;
        *end_b = *start_b;
        return true;
    }
    if (vv + vd + vd + dd) > 0.0 {
        if vd >= 0.0 {
            return false;
        }
        if -vd >= vv {
            return false;
        }
    }
    let t = -(vd + sqrt(discriminant)) / vv;
    end_a.x = start_a.x + t * (end_a.x - start_a.x);
    end_a.y = start_a.y + t * (end_a.y - start_a.y);
    end_a.z = start_a.z + t * (end_a.z - start_a.z);
    end_b.x = start_b.x + t * (end_b.x - start_b.x);
    end_b.y = start_b.y + t * (end_b.y - start_b.y);
    end_b.z = start_b.z + t * (end_b.z - start_b.z);
    true
}

/// Ball-plane collision (`collideBallWithPlane`).
pub fn collide_ball_with_plane(ball: &mut PhysBall, plane: &Plane) {
    let plane_delta_x = ball.pos.x - plane.point.x;
    let plane_delta_y = ball.pos.y - plane.point.y;
    let plane_delta_z = ball.pos.z - plane.point.z;
    let plane_dist = plane_delta_x * plane.normal.x
        + plane_delta_y * plane.normal.y
        + plane_delta_z * plane.normal.z;
    if plane_dist > ball.radius {
        return;
    }

    let mut is_hardest = -1;
    if ball.flags & COLI_OCCURRED == 0 {
        is_hardest = 0;
    }

    let penetration = ball.radius - plane_dist;
    ball.pos.x += plane.normal.x * penetration;
    ball.pos.y += plane.normal.y * penetration;
    ball.pos.z += plane.normal.z * penetration;

    let normal_speed =
        plane.normal.x * ball.vel.x + plane.normal.y * ball.vel.y + plane.normal.z * ball.vel.z;

    if normal_speed < 0.0 && ball.restitution > FLT_EPSILON {
        if normal_speed < ball.hardest_coli_speed {
            ball.hardest_coli_speed = normal_speed;
            ball.hardest_coli_anim_group_id = ball.anim_group_id;
            is_hardest = 0;
        }

        let parallel_x = ball.vel.x - plane.normal.x * normal_speed;
        let parallel_y = ball.vel.y - plane.normal.y * normal_speed;
        let parallel_z = ball.vel.z - plane.normal.z * normal_speed;
        ball.vel.x -= parallel_x * ball.friction;
        ball.vel.y -= parallel_y * ball.friction;
        ball.vel.z -= parallel_z * ball.friction;

        if normal_speed >= -5.0 * ball.gravity_accel {
            ball.vel.x -= plane.normal.x * normal_speed;
            ball.vel.y -= plane.normal.y * normal_speed;
            ball.vel.z -= plane.normal.z * normal_speed;
        } else {
            let adjusted = normal_speed + 5.0 * ball.gravity_accel;
            ball.vel.x -= plane.normal.x * (-5.0 * ball.gravity_accel);
            ball.vel.y -= plane.normal.y * (-5.0 * ball.gravity_accel);
            ball.vel.z -= plane.normal.z * (-5.0 * ball.gravity_accel);
            ball.vel.x -= (1.0 + ball.restitution) * (plane.normal.x * adjusted);
            ball.vel.y -= (1.0 + ball.restitution) * (plane.normal.y * adjusted);
            ball.vel.z -= (1.0 + ball.restitution) * (plane.normal.z * adjusted);
        }
    }

    if is_hardest == 0 {
        ball.hardest_coli_plane = *plane;
    }

    ball.flags |= COLI_OCCURRED;
}

/// Surface of the stage-92 bonus wave at (x, z), if inside its footprint.
pub fn bonus_wave_surface_at(x: f64, z: f64, timer_frames: f64) -> Option<Plane> {
    if !(-BONUS_WAVE_LIMIT..=BONUS_WAVE_LIMIT).contains(&x)
        || !(-BONUS_WAVE_LIMIT..=BONUS_WAVE_LIMIT).contains(&z)
    {
        return None;
    }
    let dist = sqrt(sum_sq2(x, z));
    let amplitude = BONUS_WAVE_AMPLITUDE_BASE + BONUS_WAVE_AMPLITUDE_SLOPE * dist;
    let angle = (BONUS_WAVE_ANGLE_SPEED * (timer_frames - BONUS_WAVE_START_FRAME)
        + BONUS_WAVE_ANGLE_SCALE * dist)
        .trunc();
    if angle > 0.0 {
        return Some(Plane {
            point: Vec3::new(x, 0.0, z),
            normal: Vec3::new(0.0, 1.0, 0.0),
        });
    }
    let angle = to_int32(angle);
    let y = sin_s16(angle) * amplitude;
    let mut normal = Vec3::new(0.0, 1.0, 0.0);
    let len_sq = sum_sq2(x, z);
    if len_sq > FLT_EPSILON {
        let scale = -(cos_s16(angle) * amplitude) * rsqrt(len_sq);
        normal.x = x * scale;
        normal.z = z * scale;
        let normal_len_sq = sum_sq3(normal.x, normal.y, normal.z);
        if normal_len_sq > FLT_EPSILON {
            let inv = rsqrt(normal_len_sq);
            normal.x *= inv;
            normal.y *= inv;
            normal.z *= inv;
        }
    }
    Some(Plane {
        point: Vec3::new(x, y, z),
        normal,
    })
}

/// Holds the matrix stack the collision routines work in (the reference keeps one per module).
#[derive(Debug, Clone, Default)]
pub struct Collider {
    pub stack: MatrixStack,
}

impl Collider {
    pub fn new() -> Collider {
        Collider {
            stack: MatrixStack::new(),
        }
    }

    /// `tfPhysballToAnimGroupSpace`: move the ball from its current group's space into `anim_group_id`'s.
    pub fn tf_physball_to_group(
        &mut self,
        ball: &mut PhysBall,
        anim_group_id: usize,
        groups: &[AnimGroupState],
    ) {
        let stack = &mut self.stack;
        if ball.anim_group_id > 0 {
            let group = &groups[ball.anim_group_id];
            let mut delta = Vec3::new(
                ball.pos.x - ball.prev_pos.x,
                ball.pos.y - ball.prev_pos.y,
                ball.pos.z - ball.prev_pos.z,
            );
            stack.from_mtx(&group.transform);
            ball.pos = stack.tf_point(&ball.pos);
            ball.vel = stack.tf_vec(&ball.vel);
            delta = stack.tf_vec(&delta);
            stack.from_mtx(&group.prev_transform);
            ball.prev_pos = stack.tf_point(&ball.prev_pos);

            ball.vel.x += ball.pos.x - ball.prev_pos.x - delta.x;
            ball.vel.y += ball.pos.y - ball.prev_pos.y - delta.y;
            ball.vel.z += ball.pos.z - ball.prev_pos.z - delta.z;
        }

        if anim_group_id > 0 {
            let group = &groups[anim_group_id];
            let mut delta = Vec3::new(
                ball.pos.x - ball.prev_pos.x,
                ball.pos.y - ball.prev_pos.y,
                ball.pos.z - ball.prev_pos.z,
            );
            stack.from_mtx(&group.transform);
            ball.pos = stack.rigid_inv_tf_point(&ball.pos);
            ball.vel = stack.rigid_inv_tf_vec(&ball.vel);
            delta = stack.rigid_inv_tf_vec(&delta);
            stack.from_mtx(&group.prev_transform);
            ball.prev_pos = stack.rigid_inv_tf_point(&ball.prev_pos);

            ball.vel.x += ball.pos.x - ball.prev_pos.x - delta.x;
            ball.vel.y += ball.pos.y - ball.prev_pos.y - delta.y;
            ball.vel.z += ball.pos.z - ball.prev_pos.z - delta.z;
        }

        ball.anim_group_id = anim_group_id;
    }

    fn tf_ball_to_local(&mut self, ball: &mut PhysBall) {
        ball.pos = self.stack.rigid_inv_tf_point(&ball.pos);
        ball.prev_pos = self.stack.rigid_inv_tf_point(&ball.prev_pos);
        ball.vel = self.stack.rigid_inv_tf_vec(&ball.vel);
    }

    fn tf_ball_to_world(&mut self, ball: &mut PhysBall) {
        ball.pos = self.stack.tf_point(&ball.pos);
        ball.prev_pos = self.stack.tf_point(&ball.prev_pos);
        ball.vel = self.stack.tf_vec(&ball.vel);
    }

    /// `testLineIntersectsRect`
    pub fn test_line_intersects_rect(
        &mut self,
        line_start: &Vec3,
        line_end: &Vec3,
        rect: &TriggerRect,
    ) -> bool {
        let stack = &mut self.stack;
        stack.from_translate(&rect.pos);
        stack.rotate_z(rect.rot[2]);
        stack.rotate_y(rect.rot[1]);
        stack.rotate_x(rect.rot[0]);
        let start = stack.rigid_inv_tf_point(line_start);
        let end = stack.rigid_inv_tf_point(line_end);

        if (end.z < 0.0 && start.z < 0.0) || (end.z > 0.0 && start.z > 0.0) {
            return false;
        }

        let mut plane_point = Vec3::new(start.x - end.x, start.y - end.y, start.z - end.z);
        if plane_point.z > FLT_EPSILON {
            plane_point.x = end.x - plane_point.x * (end.z / plane_point.z);
            plane_point.y = end.y - plane_point.y * (end.z / plane_point.z);
        } else {
            plane_point.x = end.x;
            plane_point.y = end.y;
        }

        let half_width = 0.5 * rect.width;
        let half_height = 0.5 * rect.height;
        if plane_point.x < -half_width || plane_point.x > half_width {
            return false;
        }
        if plane_point.y < -half_height || plane_point.y > half_height {
            return false;
        }
        true
    }

    /// `raycastStageDown`: highest surface at or below `pos`.
    pub fn raycast_stage_down(
        &mut self,
        pos: &Vec3,
        geom: &StageGeom,
        groups: &[AnimGroupState],
    ) -> Option<RayHit> {
        let stack = &mut self.stack;
        let mut best: Option<RayHit> = None;
        let mut best_y = f64::NEG_INFINITY;
        for (group_id, (ag, info)) in geom.groups.iter().zip(groups.iter()).enumerate() {
            let mut local_pos = *pos;
            if group_id != 0 {
                stack.from_mtx(&info.transform);
                local_pos = stack.rigid_inv_tf_point(&local_pos);
            }
            let Some(cell_tris) = ag.grid.lookup(local_pos.x, local_pos.z) else {
                continue;
            };
            for &tri_index in cell_tris {
                let tri = &ag.triangles[tri_index as usize];
                stack.from_translate(&tri.pos);
                stack.rotate_y(tri.rot[1]);
                stack.rotate_x(tri.rot[0]);
                stack.rotate_z(tri.rot[2]);
                let tri_pos = stack.rigid_inv_tf_point(&local_pos);
                let tri_dir = stack.rigid_inv_tf_vec(&Vec3::new(0.0, -1.0, 0.0));
                if tri_dir.z.abs() <= FLT_EPSILON {
                    continue;
                }
                let t = -tri_pos.z / tri_dir.z;
                if t < 0.0 {
                    continue;
                }
                let hit_x = tri_pos.x + tri_dir.x * t;
                let hit_y = tri_pos.y + tri_dir.y * t;
                if dumb_dot(0.0, 1.0, hit_x, hit_y) < -FLT_EPSILON {
                    continue;
                }
                if ((hit_x - tri.vert2[0]) * tri.edge2_normal[0])
                    + ((hit_y - tri.vert2[1]) * tri.edge2_normal[1])
                    < -FLT_EPSILON
                {
                    continue;
                }
                if ((hit_x - tri.vert3[0]) * tri.edge3_normal[0])
                    + ((hit_y - tri.vert3[1]) * tri.edge3_normal[1])
                    < -FLT_EPSILON
                {
                    continue;
                }
                let hit_local = stack.tf_point(&Vec3::new(hit_x, hit_y, 0.0));
                let hit_world = if group_id != 0 {
                    stack.from_mtx(&info.transform);
                    stack.tf_point(&hit_local)
                } else {
                    hit_local
                };
                if hit_world.y > best_y && hit_world.y <= pos.y + FLT_EPSILON {
                    best_y = hit_world.y;
                    let (normal, surface_vel) = if group_id != 0 {
                        stack.from_mtx(&info.transform);
                        let normal_world = stack.tf_vec(&tri.normal);
                        stack.from_mtx(&info.prev_transform);
                        let prev = stack.tf_point(&hit_local);
                        stack.from_mtx(&info.transform);
                        let curr = stack.tf_point(&hit_local);
                        (
                            normal_world,
                            Vec3::new(curr.x - prev.x, curr.y - prev.y, curr.z - prev.z),
                        )
                    } else {
                        (tri.normal, Vec3::ZERO)
                    };
                    best = Some(RayHit {
                        pos: hit_world,
                        normal,
                        surface_vel,
                        flags: 1,
                    });
                }
            }
        }
        best
    }

    fn collide_ball_with_tri_face(&mut self, ball: &mut PhysBall, tri: &Tri) {
        let stack = &mut self.stack;
        let mut prev = ball.prev_pos;
        let mut pos = ball.pos;

        let mut x = prev.x - tri.pos.x;
        let mut y = prev.y - tri.pos.y;
        let mut z = prev.z - tri.pos.z;
        if x * tri.normal.x + y * tri.normal.y + z * tri.normal.z < 0.0 {
            return;
        }
        x = pos.x - tri.pos.x;
        y = pos.y - tri.pos.y;
        z = pos.z - tri.pos.z;
        if x * tri.normal.x + y * tri.normal.y + z * tri.normal.z > ball.radius {
            return;
        }

        stack.from_translate(&tri.pos);
        stack.rotate_y(tri.rot[1]);
        stack.rotate_x(tri.rot[0]);
        stack.rotate_z(tri.rot[2]);
        prev = stack.rigid_inv_tf_point(&prev);
        pos = stack.rigid_inv_tf_point(&pos);

        let outside = |p: &Vec3| {
            (dumb_dot(0.0, 1.0, p.x, p.y) < -FLT_EPSILON)
                || (((p.x - tri.vert2[0]) * tri.edge2_normal[0])
                    + ((p.y - tri.vert2[1]) * tri.edge2_normal[1])
                    < -FLT_EPSILON)
                || (((p.x - tri.vert3[0]) * tri.edge3_normal[0])
                    + ((p.y - tri.vert3[1]) * tri.edge3_normal[1])
                    < -FLT_EPSILON)
        };
        let prev_outside = outside(&prev);
        let pos_outside = outside(&pos);

        if !(prev_outside && pos_outside) {
            let plane = Plane {
                point: tri.pos,
                normal: tri.normal,
            };
            collide_ball_with_plane(ball, &plane);
        }
    }

    fn collide_ball_with_tri_edge(
        &mut self,
        ball: &mut PhysBall,
        ball_pos_tri: &Vec3,
        ball_prev_pos_tri: &Vec3,
        edge: &Edge,
    ) {
        {
            let stack = &mut self.stack;
            stack.push();
            stack.from_identity();
            stack.translate_xyz(edge.start[0], edge.start[1], 0.0);
            let edge_normal_len_sq = sum_sq2(edge.normal[0], edge.normal[1]);
            if edge_normal_len_sq > FLT_EPSILON {
                stack.rotate_z(-atan2_s16(edge.normal[0], edge.normal[1]));
            }

            let mut local_prev = stack.rigid_inv_tf_point(ball_prev_pos_tri);
            let mut local_pos = stack.rigid_inv_tf_point(ball_pos_tri);

            let end_pos = stack.rigid_inv_tf_point(&Vec3::new(edge.end[0], edge.end[1], 0.0));

            let mut plane_vec =
                Vec3::new(0.0, local_pos.y - local_prev.y, local_pos.z - local_prev.z);
            let plane_len_sq = sum_sq2(plane_vec.y, plane_vec.z);
            if plane_len_sq > FLT_EPSILON {
                stack.rotate_x(-atan2_s16(plane_vec.y, plane_vec.z) - 0x8000);
            }
            local_pos = stack.rigid_inv_tf_point(ball_pos_tri);
            local_prev = stack.rigid_inv_tf_point(ball_prev_pos_tri);

            let some_y = local_pos.y;
            if some_y.abs() > ball.radius {
                stack.pop();
                return;
            }
            let mut phi = sqrt((ball.radius * ball.radius) - (local_pos.y * local_pos.y));
            if local_pos.z.abs() > phi {
                stack.pop();
                return;
            }
            if local_prev.z < 0.0 {
                phi = -phi;
            }
            plane_vec = Vec3::new(0.0, some_y, phi);
            let edge_hit_x = if local_prev.z > phi && local_pos.z < phi {
                local_prev.x
                    + ((local_pos.x - local_prev.x)
                        * ((phi - local_prev.z) / (local_pos.z - local_prev.z)))
            } else {
                local_pos.x
            };
            if edge_hit_x < 0.0 || edge_hit_x > end_pos.x {
                stack.pop();
                return;
            }

            let temp_len_sq = sum_sq2(plane_vec.y, plane_vec.z);
            if temp_len_sq <= FLT_EPSILON {
                stack.pop();
                return;
            }
            let inv_len = rsqrt(temp_len_sq);
            plane_vec.y *= inv_len;
            plane_vec.z *= inv_len;

            let mut normal = stack.tf_vec(&plane_vec);
            let mut point = stack.get_translate();

            stack.pop();
            normal = stack.tf_vec(&normal);
            point = stack.tf_point(&point);
            let plane = Plane { point, normal };
            collide_ball_with_plane(ball, &plane);
        }
    }

    fn collide_ball_with_tri_edges(&mut self, ball: &mut PhysBall, tri: &Tri) {
        let mut prev = ball.prev_pos;
        let mut pos = ball.pos;

        let mut x = prev.x - tri.pos.x;
        let mut y = prev.y - tri.pos.y;
        let mut z = prev.z - tri.pos.z;
        if x * tri.normal.x + y * tri.normal.y + z * tri.normal.z < 0.0 {
            return;
        }
        x = pos.x - tri.pos.x;
        y = pos.y - tri.pos.y;
        z = pos.z - tri.pos.z;
        if x * tri.normal.x + y * tri.normal.y + z * tri.normal.z > ball.radius {
            return;
        }

        {
            let stack = &mut self.stack;
            stack.from_translate(&tri.pos);
            stack.rotate_y(tri.rot[1]);
            stack.rotate_x(tri.rot[0]);
            stack.rotate_z(tri.rot[2]);
            prev = stack.rigid_inv_tf_point(&prev);
            pos = stack.rigid_inv_tf_point(&pos);
        }

        x = 0.0;
        y = 1.0;
        if (x * pos.x + y * pos.y < -ball.radius) && (x * prev.x + y * prev.y < -ball.radius) {
            return;
        }
        x = tri.edge2_normal[0];
        y = tri.edge2_normal[1];
        if (x * (pos.x - tri.vert2[0]) + y * (pos.y - tri.vert2[1]) < -ball.radius)
            && (x * (prev.x - tri.vert2[0]) + y * (prev.y - tri.vert2[1]) < -ball.radius)
        {
            return;
        }
        x = tri.edge3_normal[0];
        y = tri.edge3_normal[1];
        if (x * (pos.x - tri.vert3[0]) + y * (pos.y - tri.vert3[1]) < -ball.radius)
            && (x * (prev.x - tri.vert3[0]) + y * (prev.y - tri.vert3[1]) < -ball.radius)
        {
            return;
        }

        let edge = Edge {
            start: [0.0, 0.0],
            end: tri.vert2,
            normal: [0.0, 1.0],
        };
        self.collide_ball_with_tri_edge(ball, &pos, &prev, &edge);
        let edge = Edge {
            start: tri.vert2,
            end: tri.vert3,
            normal: tri.edge2_normal,
        };
        self.collide_ball_with_tri_edge(ball, &pos, &prev, &edge);
        let edge = Edge {
            start: tri.vert3,
            end: [0.0, 0.0],
            normal: tri.edge3_normal,
        };
        self.collide_ball_with_tri_edge(ball, &pos, &prev, &edge);
    }

    fn collide_ball_with_tri_vert(
        &mut self,
        ball: &mut PhysBall,
        ball_pos_tri: &Vec3,
        vert: [f64; 2],
    ) {
        let mut tmp = Vec3::new(
            ball_pos_tri.x - vert[0],
            ball_pos_tri.y - vert[1],
            ball_pos_tri.z,
        );
        let dist_sq = sum_sq3(tmp.x, tmp.y, tmp.z);
        if dist_sq > ball.radius * ball.radius || dist_sq <= FLT_EPSILON {
            return;
        }
        let inv_dist = rsqrt(dist_sq);
        tmp.x *= inv_dist;
        tmp.y *= inv_dist;
        tmp.z *= inv_dist;

        let stack = &mut self.stack;
        let normal = stack.tf_vec(&tmp);
        let point = stack.tf_point(&Vec3::new(vert[0], vert[1], 0.0));
        stack.push();
        collide_ball_with_plane(ball, &Plane { point, normal });
        stack.pop();
    }

    fn collide_ball_with_tri_verts(&mut self, ball: &mut PhysBall, tri: &Tri) {
        let prev = ball.prev_pos;
        let mut x = prev.x - tri.pos.x;
        let mut y = prev.y - tri.pos.y;
        let mut z = prev.z - tri.pos.z;
        if x * tri.normal.x + y * tri.normal.y + z * tri.normal.z < 0.0 {
            return;
        }
        let pos = ball.pos;
        x = pos.x - tri.pos.x;
        y = pos.y - tri.pos.y;
        z = pos.z - tri.pos.z;
        if x * tri.normal.x + y * tri.normal.y + z * tri.normal.z > ball.radius {
            return;
        }

        let pos_tri;
        {
            let stack = &mut self.stack;
            stack.from_translate(&tri.pos);
            stack.rotate_y(tri.rot[1]);
            stack.rotate_x(tri.rot[0]);
            stack.rotate_z(tri.rot[2]);
            pos_tri = stack.rigid_inv_tf_point(&pos);
        }
        self.collide_ball_with_tri_vert(ball, &pos_tri, [0.0, 0.0]);
        self.collide_ball_with_tri_vert(ball, &pos_tri, tri.vert2);
        self.collide_ball_with_tri_vert(ball, &pos_tri, tri.vert3);
    }

    fn collide_ball_with_rect(&mut self, ball: &mut PhysBall, rect: &Rect) {
        let stack = &mut self.stack;
        let pos = ball.pos;
        let prev = ball.prev_pos;

        let mut tmp = Vec3::new(
            prev.x - rect.pos.x,
            prev.y - rect.pos.y,
            prev.z - rect.pos.z,
        );
        if tmp.x * rect.normal.x + tmp.y * rect.normal.y + tmp.z * rect.normal.z < 0.0 {
            return;
        }
        tmp = Vec3::new(pos.x - rect.pos.x, pos.y - rect.pos.y, pos.z - rect.pos.z);
        if tmp.x * rect.normal.x + tmp.y * rect.normal.y + tmp.z * rect.normal.z > ball.radius {
            return;
        }

        stack.from_translate(&rect.pos);
        stack.rotate_y(atan2_s16(rect.normal.x, rect.normal.z));
        stack.rotate_x(-atan2_s16(
            rect.normal.y,
            sqrt(sum_sq2(rect.normal.x, rect.normal.z)),
        ));
        let local = stack.rigid_inv_tf_point(&pos);

        let half_width = 0.5 * rect.width;
        let half_height = 0.5 * rect.height;

        if local.x < -half_width - ball.radius
            || local.x > half_width + ball.radius
            || local.y < -half_height - ball.radius
            || local.y > half_height + ball.radius
        {
            return;
        }

        let mut plane = Plane {
            point: Vec3::ZERO,
            normal: Vec3::ZERO,
        };
        if local.x < -half_width {
            tmp.x = local.x + half_width;
            tmp.z = local.z;
            let dist_sq = sum_sq2(tmp.x, tmp.z);
            if dist_sq > ball.radius * ball.radius {
                return;
            }
            if dist_sq > FLT_EPSILON {
                let inv = rsqrt(dist_sq);
                tmp.x *= inv;
                tmp.y = 0.0;
                tmp.z *= inv;
            } else {
                tmp = Vec3::new(-1.0, 0.0, 0.0);
            }
            plane.normal = stack.tf_vec(&tmp);
            plane.point = stack.tf_point(&Vec3::new(-half_width, 0.0, 0.0));
        } else if local.x > half_width {
            tmp.x = local.x - half_width;
            tmp.z = local.z;
            let dist_sq = sum_sq2(tmp.x, tmp.z);
            if dist_sq > ball.radius * ball.radius {
                return;
            }
            if dist_sq > FLT_EPSILON {
                let inv = rsqrt(dist_sq);
                tmp.x *= inv;
                tmp.y = 0.0;
                tmp.z *= inv;
            } else {
                tmp = Vec3::new(1.0, 0.0, 0.0);
            }
            plane.normal = stack.tf_vec(&tmp);
            plane.point = stack.tf_point(&Vec3::new(half_width, 0.0, 0.0));
        } else if local.y < -half_height {
            tmp.y = local.y + half_height;
            tmp.z = local.z;
            let dist_sq = sum_sq2(tmp.y, tmp.z);
            if dist_sq > ball.radius * ball.radius {
                return;
            }
            if dist_sq > FLT_EPSILON {
                let inv = rsqrt(dist_sq);
                tmp.x = 0.0;
                tmp.y *= inv;
                tmp.z *= inv;
            } else {
                tmp = Vec3::new(0.0, -1.0, 0.0);
            }
            plane.normal = stack.tf_vec(&tmp);
            plane.point = stack.tf_point(&Vec3::new(0.0, -half_height, 0.0));
        } else if local.y > half_height {
            tmp.y = local.y - half_height;
            tmp.z = local.z;
            let dist_sq = sum_sq2(tmp.y, tmp.z);
            if dist_sq > ball.radius * ball.radius {
                return;
            }
            if dist_sq > FLT_EPSILON {
                let inv = rsqrt(dist_sq);
                tmp.x = 0.0;
                tmp.y *= inv;
                tmp.z *= inv;
            } else {
                tmp = Vec3::new(0.0, 1.0, 0.0);
            }
            plane.normal = stack.tf_vec(&tmp);
            plane.point = stack.tf_point(&Vec3::new(0.0, half_height, 0.0));
        } else {
            plane.point = rect.pos;
            plane.normal = rect.normal;
        }
        collide_ball_with_plane(ball, &plane);
    }

    fn collide_ball_with_jamabar(&mut self, ball: &mut PhysBall, jamabar: &mut JamabarState) {
        self.stack.from_translate(&jamabar.pos);
        self.stack.rotate_x(jamabar.rot[0]);
        self.stack.rotate_y(jamabar.rot[1]);
        self.stack.rotate_z(jamabar.rot[2]);
        self.tf_ball_to_local(ball);
        let ball_pos = ball.pos;

        for rect in JAMABAR_RECTS.iter() {
            self.collide_ball_with_rect(ball, rect);
        }

        let delta = 0.75 * (ball.pos.z - ball_pos.z);
        jamabar.local_pos.z -= delta;
        ball.pos.z += delta;
        let local_vel = jamabar.local_vel.z;
        if delta * local_vel > 0.0 {
            jamabar.local_vel.z *= 0.5;
            ball.vel.z += 2.5 * (local_vel - jamabar.local_vel.z);
        }

        self.stack.from_translate(&jamabar.pos);
        self.stack.rotate_x(jamabar.rot[0]);
        self.stack.rotate_y(jamabar.rot[1]);
        self.stack.rotate_z(jamabar.rot[2]);
        self.tf_ball_to_world(ball);
    }

    fn collide_ball_with_cylinder(&mut self, ball: &mut PhysBall, cylinder: &Cylinder) {
        let tmp = Vec3::new(
            ball.pos.x - cylinder.pos.x,
            ball.pos.y - cylinder.pos.y,
            ball.pos.z - cylinder.pos.z,
        );
        let dist_sq = sum_sq3(tmp.x, tmp.y, tmp.z);
        let cull_radius = ball.radius + sqrt(sum_sq2(cylinder.radius, cylinder.height));
        if dist_sq > cull_radius * cull_radius {
            return;
        }

        self.stack.from_translate(&cylinder.pos);
        self.stack.rotate_z(cylinder.rot[2]);
        self.stack.rotate_y(cylinder.rot[1]);
        self.stack.rotate_x(cylinder.rot[0]);

        let pos_local = self.stack.rigid_inv_tf_point(&ball.pos);
        let prev_local = self.stack.rigid_inv_tf_point(&ball.prev_pos);

        if prev_local.y < 0.5 * -cylinder.height {
            let circle = Circle {
                pos: self
                    .stack
                    .tf_point(&Vec3::new(0.0, 0.5 * -cylinder.height, 0.0)),
                radius: cylinder.radius,
                rot: [cylinder.rot[0] - 0x8000, cylinder.rot[1], cylinder.rot[2]],
            };
            self.collide_ball_with_circle(ball, &circle);
            return;
        }
        if prev_local.y > 0.5 * cylinder.height {
            let circle = Circle {
                pos: self
                    .stack
                    .tf_point(&Vec3::new(0.0, 0.5 * cylinder.height, 0.0)),
                radius: cylinder.radius,
                rot: cylinder.rot,
            };
            self.collide_ball_with_circle(ball, &circle);
            return;
        }

        let dist2d = sum_sq2(pos_local.x, pos_local.z);
        let radius_sum = cylinder.radius + ball.radius;
        if dist2d > radius_sum * radius_sum || dist2d < FLT_EPSILON {
            return;
        }

        let inv_len = rsqrt(dist2d);
        let point = Vec3::new(
            pos_local.x * inv_len * cylinder.radius,
            pos_local.y,
            pos_local.z * inv_len * cylinder.radius,
        );
        let normal = Vec3::new(pos_local.x * inv_len, 0.0, pos_local.z * inv_len);
        let plane = Plane {
            normal: self.stack.tf_vec(&normal),
            point: self.stack.tf_point(&point),
        };
        collide_ball_with_plane(ball, &plane);
    }

    fn collide_ball_with_circle(&mut self, ball: &mut PhysBall, circle: &Circle) {
        let mut tmp = Vec3::new(
            ball.pos.x - circle.pos.x,
            ball.pos.y - circle.pos.y,
            ball.pos.z - circle.pos.z,
        );
        let dist_sq = sum_sq3(tmp.x, tmp.y, tmp.z);
        let radius_sum = ball.radius + circle.radius;
        if dist_sq > radius_sum * radius_sum {
            return;
        }

        let stack = &mut self.stack;
        stack.from_translate(&circle.pos);
        stack.rotate_z(circle.rot[2]);
        stack.rotate_y(circle.rot[1]);
        stack.rotate_x(circle.rot[0]);
        let ball_pos_circle = stack.rigid_inv_tf_point(&ball.pos);

        let dist2d = sum_sq2(ball_pos_circle.x, ball_pos_circle.z);
        if dist2d < circle.radius * circle.radius {
            let plane = Plane {
                point: stack.get_translate(),
                normal: stack.tf_vec(&Vec3::new(0.0, 1.0, 0.0)),
            };
            collide_ball_with_plane(ball, &plane);
            return;
        }

        let radius_sum_sq = radius_sum * radius_sum;
        if dist2d < radius_sum_sq && dist2d > FLT_EPSILON {
            let temp = circle.radius * rsqrt(dist2d);
            let plane_point = Vec3::new(ball_pos_circle.x * temp, 0.0, ball_pos_circle.z * temp);
            tmp.x = ball_pos_circle.x - plane_point.x;
            tmp.y = ball_pos_circle.y - plane_point.y;
            tmp.z = ball_pos_circle.z - plane_point.z;
            let dist_sq2 = sum_sq3(tmp.x, tmp.y, tmp.z);
            if dist_sq2 > FLT_EPSILON {
                let inv_len = rsqrt(dist_sq2);
                let normal = Vec3::new(tmp.x * inv_len, tmp.y * inv_len, tmp.z * inv_len);
                let plane = Plane {
                    point: stack.tf_point(&plane_point),
                    normal: stack.tf_vec(&normal),
                };
                collide_ball_with_plane(ball, &plane);
            }
        }
    }

    fn collide_ball_with_sphere(&mut self, ball: &mut PhysBall, sphere: &Sphere) {
        let mut tmp = Vec3::new(
            ball.pos.x - sphere.pos.x,
            ball.pos.y - sphere.pos.y,
            ball.pos.z - sphere.pos.z,
        );
        let dist_sq = sum_sq3(tmp.x, tmp.y, tmp.z);
        let radius_sum = sphere.radius + ball.radius;
        if dist_sq > radius_sum * radius_sum || dist_sq <= FLT_EPSILON {
            return;
        }
        let inv_dist = rsqrt(dist_sq);
        tmp.x *= inv_dist;
        tmp.y *= inv_dist;
        tmp.z *= inv_dist;
        let plane = Plane {
            point: Vec3::new(
                sphere.pos.x + tmp.x * sphere.radius,
                sphere.pos.y + tmp.y * sphere.radius,
                sphere.pos.z + tmp.z * sphere.radius,
            ),
            normal: tmp,
        };
        collide_ball_with_plane(ball, &plane);
    }

    fn collide_ball_with_cone(&mut self, ball: &mut PhysBall, cone: &Cone) {
        let tmp = Vec3::new(
            ball.pos.x - cone.pos.x,
            ball.pos.y - cone.pos.y,
            ball.pos.z - cone.pos.z,
        );
        let dist_sq = sum_sq3(tmp.x, tmp.y, tmp.z);
        let max_scale = if cone.scale.x > cone.scale.y {
            cone.scale.x
        } else {
            cone.scale.y
        };
        let cull_radius = max_scale + ball.radius;
        if dist_sq > cull_radius * cull_radius {
            return;
        }

        let stack = &mut self.stack;
        stack.from_translate(&cone.pos);
        stack.rotate_z(cone.rot[2]);
        stack.rotate_y(cone.rot[1]);
        stack.rotate_x(cone.rot[0]);
        let ball_pos_cone = stack.rigid_inv_tf_point(&ball.pos);
        if ball_pos_cone.y < -ball.radius || ball_pos_cone.y > cone.scale.y + ball.radius {
            return;
        }

        let dist2d = sum_sq2(ball_pos_cone.x, ball_pos_cone.z);
        let cyl_cull_radius = cone.scale.x + ball.radius;
        if dist2d > cyl_cull_radius * cyl_cull_radius {
            return;
        }

        stack.translate_xyz(0.0, cone.scale.y, 0.0);
        stack.rotate_y(-atan2_s16(ball_pos_cone.z, ball_pos_cone.x));
        stack.rotate_z(-atan2_s16(cone.scale.y, cone.scale.x));
        let tmp = stack.rigid_inv_tf_point(&ball.pos);

        if tmp.x < 0.0 {
            let point = stack.get_translate();
            let len_sq = sum_sq3(tmp.x, tmp.y, tmp.z);
            if len_sq <= FLT_EPSILON {
                return;
            }
            let inv_len = 1.0 / sqrt(len_sq);
            let normal = Vec3::new(tmp.x * inv_len, tmp.y * inv_len, tmp.z * inv_len);
            let plane = Plane {
                point,
                normal: stack.tf_vec(&normal),
            };
            collide_ball_with_plane(ball, &plane);
        } else {
            let base_radius = sqrt(sum_sq2(cone.scale.x, cone.scale.y));
            if tmp.x > ball.radius + base_radius {
                return;
            }
            let plane = Plane {
                point: stack.get_translate(),
                normal: stack.tf_vec(&Vec3::new(0.0, 1.0, 0.0)),
            };
            collide_ball_with_plane(ball, &plane);
        }
    }

    fn goal_sub22(&mut self, ball: &mut PhysBall, info: &RingInfo) {
        let max_dist = info.radius + info.thickness + ball.radius;
        let min_dist = info.radius - info.thickness - ball.radius;
        let dist = vec_distance(&ball.pos, &info.center);
        if dist > max_dist || dist < min_dist {
            return;
        }

        let stack = &mut self.stack;
        stack.from_translate(&info.center);
        stack.rotate_z(info.rot[2]);
        stack.rotate_y(info.rot[1]);
        stack.rotate_x(info.rot[0]);
        let pos_local = stack.rigid_inv_tf_point(&ball.pos);
        let prev_local = stack.rigid_inv_tf_point(&ball.prev_pos);

        let ring_point = Vec3::new(pos_local.x + prev_local.x, pos_local.y + prev_local.y, 0.0);
        let mut ring_point = vec_set_len(&ring_point, info.radius);
        if intersects_moving_spheres(
            &prev_local,
            &pos_local,
            &ring_point,
            &ring_point,
            ball.radius,
            info.thickness,
        ) {
            let mut plane_vec = Vec3::new(
                prev_local.x - ring_point.x,
                prev_local.y - ring_point.y,
                prev_local.z - ring_point.z,
            );
            vec_normalize_len(&mut plane_vec);
            let normal = stack.tf_vec(&plane_vec);
            ring_point.x += plane_vec.x * info.thickness;
            ring_point.y += plane_vec.y * info.thickness;
            ring_point.z += plane_vec.z * info.thickness;
            let point = stack.tf_point(&ring_point);
            collide_ball_with_plane(ball, &Plane { point, normal });
        }
    }

    fn goal_sub23(&mut self, ball: &mut PhysBall, info: &RingInfo) {
        let up = self.stack.tf_vec(&Vec3::new(0.0, -1.0, 0.0));
        let mut dir = Vec3::new(
            ball.pos.x - info.center.x,
            ball.pos.y - info.center.y,
            ball.pos.z - info.center.z,
        );
        vec_normalize_len(&mut dir);
        if vec_dot(&up, &dir) <= GOAL_RING_MIN_DOT || vec_dot(&up, &dir).is_nan() {
            self.goal_sub22(ball, info);
        }
    }

    fn goal_sub24(&mut self, ball: &mut PhysBall, info: &RingInfo) {
        let max_dist = info.radius + info.thickness + ball.radius;
        if vec_distance(&ball.pos, &info.center) > max_dist {
            return;
        }

        let stack = &mut self.stack;
        stack.from_translate(&info.center);
        stack.rotate_z(info.rot[2]);
        stack.rotate_y(info.rot[1]);
        stack.rotate_x(info.rot[0]);
        let pos_local = stack.rigid_inv_tf_point(&ball.pos);
        let prev_local = stack.rigid_inv_tf_point(&ball.prev_pos);

        let mut ring_point = Vec3::new(
            (pos_local.x + prev_local.x) * 0.5,
            (pos_local.y + prev_local.y) * 0.5,
            0.0,
        );
        if ring_point.y < -0.5 {
            ring_point.y = -0.5;
        }
        let ring_len = sqrt(sum_sq2(ring_point.x, ring_point.y));
        if ring_len > info.radius && ring_len > FLT_EPSILON {
            let scale = info.radius / ring_len;
            ring_point.x *= scale;
            ring_point.y *= scale;
            ring_point.z *= scale;
        }
        if intersects_moving_spheres(
            &prev_local,
            &pos_local,
            &ring_point,
            &ring_point,
            ball.radius,
            info.thickness,
        ) {
            let mut plane_vec = Vec3::new(
                prev_local.x - ring_point.x,
                prev_local.y - ring_point.y,
                prev_local.z - ring_point.z,
            );
            vec_normalize_len(&mut plane_vec);
            let normal = stack.tf_vec(&plane_vec);
            ring_point.x += plane_vec.x * info.thickness;
            ring_point.y += plane_vec.y * info.thickness;
            ring_point.z += plane_vec.z * info.thickness;
            let point = stack.tf_point(&ring_point);
            collide_ball_with_plane(ball, &Plane { point, normal });
        }
    }

    fn collide_ball_with_goal(&mut self, ball: &mut PhysBall, goal: &GoalGeom) {
        self.stack.from_translate(&goal.pos);
        self.stack.rotate_z(goal.rot[2]);
        self.stack.rotate_y(goal.rot[1]);
        self.stack.rotate_x(goal.rot[0]);

        let ring_center = |stack: &MatrixStack| stack.tf_point(&Vec3::new(0.0, 3.2, 0.0));

        self.stack.push();
        let info = RingInfo {
            center: ring_center(&self.stack),
            radius: GOAL_RING_RADIUS,
            thickness: GOAL_RING_THICKNESS,
            rot: goal.rot,
        };
        self.goal_sub23(ball, &info);
        self.stack.pop();

        self.stack.push();
        let info = RingInfo {
            center: ring_center(&self.stack),
            radius: GOAL_RING_RADIUS,
            thickness: GOAL_RING_THICKNESS,
            rot: goal.rot,
        };
        self.goal_sub24(ball, &info);
        self.stack.pop();

        for x in [1.1, -1.1] {
            self.stack.push();
            let cyl = Cylinder {
                pos: self.stack.tf_point(&Vec3::new(x, 0.75, 0.0)),
                radius: GOAL_POST_RADIUS,
                height: GOAL_POST_HEIGHT,
                rot: goal.rot,
            };
            self.collide_ball_with_cylinder(ball, &cyl);
            self.stack.pop();
        }
    }

    fn collide_ball_with_bumper(&mut self, ball: &mut PhysBall, bumper: &mut BumperState) {
        bumper.state = 1;
        let resolve_radius = bumper.model_radius;
        let mut temp_pos = bumper.pos;
        resolve_moving_spheres(
            &ball.prev_pos,
            &mut ball.pos,
            &bumper.prev_pos,
            &mut temp_pos,
            ball.radius,
            resolve_radius,
        );
        let mut normal = Vec3::new(
            ball.pos.x - bumper.pos.x,
            ball.pos.y - bumper.pos.y,
            ball.pos.z - bumper.pos.z,
        );
        vec_normalize_len(&mut normal);
        let push = -1.5 * (normal.x * ball.vel.x + normal.y * ball.vel.y + normal.z * ball.vel.z);
        if push > 0.0 {
            ball.vel.x += push * normal.x;
            ball.vel.y += push * normal.y;
            ball.vel.z += push * normal.z;
        }
        ball.vel.x += 0.05 * normal.x;
        ball.vel.y += 0.05 * normal.y;
        ball.vel.z += 0.05 * normal.z;
        let dist = bumper.radius + ball.radius + 0.01;
        ball.pos.x = bumper.pos.x + normal.x * dist;
        ball.pos.y = bumper.pos.y + normal.y * dist;
        ball.pos.z = bumper.pos.z + normal.z * dist;
    }

    fn collide_ball_with_goal_bag(&mut self, ball: &mut PhysBall, bag: &mut GoalBagState) {
        let mut a = Vec3::new(
            ball.pos.x - bag.position.x,
            ball.pos.y - bag.position.y,
            ball.pos.z - bag.position.z,
        );
        if vec_normalize_len(&mut a) == 0.0 {
            return;
        }
        let b = Vec3::new(
            bag.position.x - bag.prev_pos.x,
            bag.position.y - bag.prev_pos.y,
            bag.position.z - bag.prev_pos.z,
        );
        let c = ball.vel;
        let mut d = Vec3::new(c.x - b.x, c.y - b.y, c.z - b.z);
        let mut var_f26 = 0.0;
        let mut temp_f0 = -1.5 * vec_dot(&a, &ball.vel);
        if temp_f0 > 0.0 {
            var_f26 = temp_f0;
            d.x += temp_f0 * a.x;
            d.y += temp_f0 * a.y;
            d.z += temp_f0 * a.z;
            ball.vel.x = d.x + b.x;
            ball.vel.y = d.y + b.y;
            ball.vel.z = d.z + b.z;
        }
        let _ = var_f26;
        let temp_f25 = bag.bound_sphere_radius + ball.radius;
        a.x *= temp_f25;
        a.y *= temp_f25;
        a.z *= temp_f25;
        ball.pos.x += bag.position.x + a.x - ball.pos.x;
        ball.pos.y += bag.position.y + a.y - ball.pos.y;
        ball.pos.z += bag.position.z + a.z - ball.pos.z;

        temp_f0 = -vec_dot(&a, &bag.local_pos);
        a.x += temp_f0 * bag.local_pos.x;
        a.y += temp_f0 * bag.local_pos.y;
        a.z += temp_f0 * bag.local_pos.z;
        vec_normalize_len(&mut a);

        d.x = b.x + bag.local_vel.x - c.x;
        d.y = b.y + bag.local_vel.y - c.y;
        d.z = b.z + bag.local_vel.z - c.z;
        temp_f0 = -2.0 * vec_dot(&a, &d);
        if temp_f0 < 0.0 {
            d.x += temp_f0 * a.x;
            d.y += temp_f0 * a.y;
            d.z += temp_f0 * a.z;
            bag.local_vel.x = c.x + (d.x - b.x);
            bag.local_vel.y = c.y + (d.y - b.y);
            bag.local_vel.z = c.z + (d.z - b.z);
        }

        temp_f0 = -vec_dot(&bag.local_pos, &bag.local_vel);
        bag.local_vel.x += temp_f0 * bag.local_pos.x;
        bag.local_vel.y += temp_f0 * bag.local_pos.y;
        bag.local_vel.z += temp_f0 * bag.local_pos.z;
        if bag.flags != 0 {
            temp_f0 = -2.0 * vec_dot(&bag.local_pos, &c);
            if temp_f0 < 0.0 {
                bag.unk8 += temp_f0;
            }
        }
    }

    fn collide_ball_with_goal_tape(&mut self, ball: &mut PhysBall, tape: &mut GoalTapeState) {
        if tape.points.is_empty() {
            return;
        }
        let stack = &mut self.stack;
        stack.from_identity();
        stack.translate(&tape.goal_pos);
        stack.rotate_z(tape.goal_rot[2]);
        stack.rotate_y(tape.goal_rot[1]);
        stack.rotate_x(tape.goal_rot[0]);
        let local_pos = stack.rigid_inv_tf_point(&ball.pos);

        let radius = ball.radius;
        let radius_sq = radius * radius;
        let mut impulse = Vec3::ZERO;
        let mut hit = false;

        for point in tape.points.iter_mut() {
            if point.flags & 1 != 0 {
                continue;
            }
            let mut delta = Vec3::ZERO;
            delta.x = point.pos.x - local_pos.x;
            if delta.x.abs() > radius {
                continue;
            }
            delta.z = point.pos.z - local_pos.z;
            if delta.z.abs() > radius {
                continue;
            }
            delta.y = point.pos.y - local_pos.y;
            if delta.y.abs() > radius {
                continue;
            }
            let dist_sq = (delta.x * delta.x) + (delta.y * delta.y) + (delta.z * delta.z);
            if dist_sq > radius_sq {
                continue;
            }

            delta = vec_set_len(&delta, radius);
            let new_x = local_pos.x + delta.x;
            let new_y = local_pos.y + delta.y;
            let new_z = local_pos.z + delta.z;
            let delta_x = new_x - point.pos.x;
            let delta_y = new_y - point.pos.y;
            let delta_z = new_z - point.pos.z;
            point.pos.x = new_x;
            point.pos.y = new_y;
            point.pos.z = new_z;
            point.vel.x += delta_x;
            point.vel.y += delta_y;
            point.vel.z += delta_z;
            hit = true;

            let move_len = sqrt((delta_x * delta_x) + (delta_y * delta_y) + (delta_z * delta_z));
            let step = vec_set_len(&delta, move_len * -0.05);
            impulse.x += step.x;
            impulse.y += step.y;
            impulse.z += step.z;

            let mut normal = point.normal;
            vec_normalize_len(&mut delta);
            if (delta.x * normal.x) + (delta.y * normal.y) + (delta.z * normal.z) < 0.0 {
                delta.x = -delta.x;
                delta.y = -delta.y;
                delta.z = -delta.z;
            }
            normal.x += (delta.x - normal.x) * 0.125;
            normal.y += (delta.y - normal.y) * 0.125;
            normal.z += (delta.z - normal.z) * 0.125;
            vec_normalize_len(&mut normal);
            point.normal = normal;
        }

        if hit {
            let impulse = stack.tf_vec(&impulse);
            ball.vel.x += impulse.x;
            ball.vel.y += impulse.y;
            ball.vel.z += impulse.z;
        }
    }

    /// `collideBallWithStageObjects`: bumpers, jamabars, goal bags and tapes of every anim group.
    pub fn collide_ball_with_stage_objects(
        &mut self,
        ball: &mut PhysBall,
        objs: &mut ObjectsMut,
        groups: &[AnimGroupState],
    ) {
        for group_id in 0..groups.len() {
            if group_id != ball.anim_group_id {
                self.tf_physball_to_group(ball, group_id, groups);
            }
            for bumper in objs.bumpers[group_id].iter_mut() {
                if intersects_moving_spheres(
                    &ball.prev_pos,
                    &ball.pos,
                    &bumper.prev_pos,
                    &bumper.pos,
                    ball.radius,
                    bumper.radius,
                ) {
                    self.collide_ball_with_bumper(ball, bumper);
                }
            }
            for jamabar in objs.jamabars[group_id].iter_mut() {
                if intersects_moving_spheres(
                    &ball.prev_pos,
                    &ball.pos,
                    &jamabar.prev_pos,
                    &jamabar.pos,
                    ball.radius,
                    jamabar.radius,
                ) {
                    self.collide_ball_with_jamabar(ball, jamabar);
                }
            }
            for &bag_index in &objs.bag_indices_by_group[group_id] {
                let bag = &mut objs.goal_bags[bag_index];
                if intersects_moving_spheres(
                    &ball.prev_pos,
                    &ball.pos,
                    &bag.prev_pos,
                    &bag.position,
                    ball.radius,
                    bag.bound_sphere_radius,
                ) {
                    self.collide_ball_with_goal_bag(ball, bag);
                }
            }
            for &tape_index in &objs.bag_indices_by_group[group_id] {
                self.collide_ball_with_goal_tape(ball, &mut objs.goal_tapes[tape_index]);
            }
        }

        if ball.anim_group_id != 0 {
            self.tf_physball_to_group(ball, 0, groups);
        }
    }

    /// `collideBallWithBonusWave` (stage 92 only).
    pub fn collide_ball_with_bonus_wave(
        &mut self,
        ball: &mut PhysBall,
        stage_id: u32,
        timer_frames: i32,
    ) {
        if stage_id != BONUS_WAVE_STAGE_ID {
            return;
        }
        if let Some(surface) = bonus_wave_surface_at(ball.pos.x, ball.pos.z, timer_frames as f64) {
            collide_ball_with_plane(ball, &surface);
        }
    }

    /// `collideBallWithStage`: static geometry of every anim group.
    pub fn collide_ball_with_stage(
        &mut self,
        ball: &mut PhysBall,
        geom: &StageGeom,
        groups: &[AnimGroupState],
    ) {
        for (group_id, ag) in geom.groups.iter().enumerate() {
            if group_id != ball.anim_group_id {
                self.tf_physball_to_group(ball, group_id, groups);
            }
            self.collide_group_geometry(ball, ag);
        }

        if ball.anim_group_id != 0 {
            self.tf_physball_to_group(ball, 0, groups);
        }
    }

    fn collide_group_geometry(&mut self, ball: &mut PhysBall, ag: &GroupGeom) {
        if let Some(cell_tris) = ag.grid.lookup(ball.pos.x, ball.pos.z) {
            for &i in cell_tris {
                self.collide_ball_with_tri_face(ball, &ag.triangles[i as usize]);
            }
            for &i in cell_tris {
                self.collide_ball_with_tri_edges(ball, &ag.triangles[i as usize]);
            }
            for &i in cell_tris {
                self.collide_ball_with_tri_verts(ball, &ag.triangles[i as usize]);
            }
        }
        for cone in &ag.cones {
            self.collide_ball_with_cone(ball, cone);
        }
        for sphere in &ag.spheres {
            self.collide_ball_with_sphere(ball, sphere);
        }
        for cylinder in &ag.cylinders {
            self.collide_ball_with_cylinder(ball, cylinder);
        }
        for goal in &ag.goals {
            self.collide_ball_with_goal(ball, goal);
        }
    }
}

struct Circle {
    pos: Vec3,
    radius: f64,
    rot: [i32; 3],
}

struct RingInfo {
    center: Vec3,
    radius: f64,
    thickness: f64,
    rot: [i32; 3],
}

/// Mutable borrow of the stage objects the object-collision pass touches.
pub struct ObjectsMut<'a> {
    pub bumpers: &'a mut [Vec<BumperState>],
    pub jamabars: &'a mut [Vec<JamabarState>],
    pub goal_bags: &'a mut [GoalBagState],
    pub goal_tapes: &'a mut [GoalTapeState],
    /// For each group, indices into `goal_bags` / `goal_tapes` (they are created in lock step per goal).
    pub bag_indices_by_group: &'a [Vec<usize>],
}

#[cfg(test)]
mod tests;

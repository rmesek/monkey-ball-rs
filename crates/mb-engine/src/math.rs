//! Port of WebMonkeyBall `src/math.ts`.
//!
//! Numeric model (matches the reference exactly): scalars and vectors are IEEE `f64` (JS numbers); a value is
//! rounded to `f32` only where the reference does so: `Float32Array` storage (the 3x4 matrices and the lookup
//! tables), `Math.fround` calls, and the custom `sqrt`/`rsqrt`. Operation order is kept identical to the
//! reference so results are bit-identical (Rust never fuses multiply-add).

use crate::tables::tables;

pub const EPSILON: f64 = 1e-7;
pub const FLT_EPSILON: f64 = 1.1920929e-7;
/// `0x10000 / 360`
pub const DEG_TO_S16: f64 = 65536.0 / 360.0;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Vec3 {
    pub const ZERO: Vec3 = Vec3 {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };
    pub const fn new(x: f64, y: f64, z: f64) -> Vec3 {
        Vec3 { x, y, z }
    }
    pub fn from_f32(v: mb_stage::Vec3) -> Vec3 {
        Vec3::new(v.x as f64, v.y as f64, v.z as f64)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quat {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub w: f64,
}

impl Quat {
    pub const IDENTITY: Quat = Quat {
        x: 0.0,
        y: 0.0,
        z: 0.0,
        w: 1.0,
    };
}

impl Default for Quat {
    fn default() -> Self {
        Quat::IDENTITY
    }
}

/// 3x4 row-major affine matrix stored as `f32` (the reference's `Float32Array(12)`).
pub type Mtx = [f32; 12];

pub const MTX_IDENTITY: Mtx = [1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0.];

/// `Math.fround`
#[inline]
pub fn f32r(v: f64) -> f64 {
    v as f32 as f64
}

/// JS `ToInt32` (used by `|0`, `&`, `>>`).
#[inline]
pub fn to_int32(v: f64) -> i32 {
    if !v.is_finite() {
        return 0;
    }
    let t = v.trunc();
    if t.abs() < 2147483648.0 {
        t as i32
    } else {
        (t.rem_euclid(4294967296.0) as u32) as i32
    }
}

/// `Math.round` (round half toward +infinity).
#[inline]
pub fn js_round(v: f64) -> f64 {
    (v + 0.5).floor()
}

/// `toS16`: truncate then wrap to the signed 16-bit range.
#[inline]
pub fn to_s16(value: f64) -> i32 {
    let v = value.trunc();
    ((to_int32(v + 32768.0)) & 0xffff) - 0x8000
}

/// Integer variant of [`to_s16`].
#[inline]
pub fn wrap_s16(v: i32) -> i32 {
    ((v.wrapping_add(0x8000)) & 0xffff) - 0x8000
}

pub fn sin_s16(angle: i32) -> f64 {
    let a = angle & 0xffff;
    let mut index = a & 0x3fff;
    if a & 0x4000 != 0 {
        index = 0x4000 - index;
    }
    let result = tables().sin[index as usize] as f64;
    if a & 0x8000 != 0 {
        -result
    } else {
        result
    }
}

pub fn cos_s16(angle: i32) -> f64 {
    sin_s16((angle.wrapping_add(0x4000)) & 0xffff)
}

fn round_to_nearest_even(value: f64) -> f64 {
    let base = value.floor();
    let frac = value - base;
    if frac > 0.5 {
        return base + 1.0;
    }
    if frac < 0.5 {
        return base;
    }
    if (base as i64) & 1 == 0 {
        base
    } else {
        base + 1.0
    }
}

fn atan_index(x: f64) -> usize {
    let bits = (x as f32).to_bits();
    let mut r5 = bits & 0x7f_ffff;
    r5 |= 0x0080_0000;
    let exp = (bits >> 23) & 0xff;
    if exp <= 0x67 {
        return 0;
    }
    let mut r4 = 0x87u32.wrapping_sub(exp);
    if r4 & !0x1f == 0 {
        r4 |= 0x20;
    }
    let shift = r4 & 31;
    let shifted = r5 >> shift;
    let offset = shifted & 0x7ff8;
    (offset >> 3) as usize
}

fn atan_s16(value: f64) -> f64 {
    if value == 0.0 {
        return 0.0;
    }
    let mut sign = 1.0;
    let mut x = value;
    if x < 0.0 {
        sign = -1.0;
        x = -x;
    }
    if x == 1.0 {
        return sign * 8192.0;
    }
    let mut invert = false;
    if x > 1.0 {
        x = 1.0 / x;
        invert = true;
    }
    x = f32r(x);
    let index = atan_index(x);
    let t = tables();
    let slope = t.atan[index * 2] as f64;
    let intercept = t.atan[index * 2 + 1] as f64;
    let angle = f32r(f32r(x * slope) + intercept);
    let mut result = round_to_nearest_even(angle);
    if invert {
        result = 16384.0 - result;
    }
    result * sign
}

/// `atan2S16(y, x)`: S16 angle in -0x8000..0x7fff.
pub fn atan2_s16(y: f64, x: f64) -> i32 {
    let yy = f32r(y);
    let xx = f32r(x);
    if yy == 0.0 && xx == 0.0 {
        return 0;
    }
    let ay = yy.abs();
    let ax = xx.abs();
    let mut angle;
    if ay == ax {
        angle = if ay == 0.0 { 0.0 } else { 8192.0 };
    } else if ay > ax {
        angle = atan_s16(ax / ay);
        angle = 16384.0 - angle;
    } else {
        angle = atan_s16(ay / ax);
    }
    if xx < 0.0 {
        if yy >= 0.0 {
            angle = 32768.0 - angle;
        } else {
            angle -= 32768.0;
        }
    } else if yy < 0.0 {
        angle = -angle;
    }
    to_s16(angle)
}

pub fn atan2_s16_safe(y: f64, x: f64) -> i32 {
    if y.abs() + x.abs() <= EPSILON {
        return 0;
    }
    atan2_s16(y, x)
}

#[inline]
pub fn sum_sq2(x: f64, y: f64) -> f64 {
    x * x + y * y
}

#[inline]
pub fn sum_sq3(x: f64, y: f64, z: f64) -> f64 {
    x * x + y * y + z * z
}

fn rsqrt_smb1(value: f64) -> f64 {
    let v = f32r(value);
    let mut x = f32r(1.0 / v.sqrt());
    let half = 0.5;
    let one_point_five = 1.5;
    for _ in 0..3 {
        let xx = f32r(x * x);
        let term = f32r(one_point_five - f32r(half * f32r(v * xx)));
        x = f32r(x * term);
    }
    x
}

/// Custom `sqrt` (f32-precision, `v * rsqrt(v)`).
pub fn sqrt(value: f64) -> f64 {
    let v = f32r(value);
    if v.is_nan() {
        return f64::NAN;
    }
    if v <= 0.0 {
        return 0.0;
    }
    if !v.is_finite() {
        return v;
    }
    let inv = rsqrt_smb1(v);
    f32r(v * inv)
}

pub fn rsqrt(value: f64) -> f64 {
    let v = f32r(value);
    if v.is_nan() {
        return f64::NAN;
    }
    if v <= 0.0 {
        return f64::INFINITY;
    }
    if !v.is_finite() {
        return 0.0;
    }
    rsqrt_smb1(v)
}

#[inline]
pub fn vec_dot(a: &Vec3, b: &Vec3) -> f64 {
    a.x * b.x + a.y * b.y + a.z * b.z
}

pub fn vec_dot_normalized(a: &Vec3, b: &Vec3) -> f64 {
    let a_len_sq = sum_sq3(a.x, a.y, a.z);
    let b_len_sq = sum_sq3(b.x, b.y, b.z);
    if a_len_sq <= EPSILON || b_len_sq <= EPSILON {
        return 0.0;
    }
    (a.x * b.x + a.y * b.y + a.z * b.z) * rsqrt(a_len_sq * b_len_sq)
}

pub fn vec_len(a: &Vec3) -> f64 {
    sqrt(sum_sq3(a.x, a.y, a.z))
}

/// Normalizes in place, returns the length (0 and untouched when degenerate).
pub fn vec_normalize_len(a: &mut Vec3) -> f64 {
    let len_sq = sum_sq3(a.x, a.y, a.z);
    if len_sq <= EPSILON {
        return 0.0;
    }
    let inv = rsqrt(len_sq);
    a.x *= inv;
    a.y *= inv;
    a.z *= inv;
    1.0 / inv
}

pub fn vec_set_len(vec: &Vec3, length: f64) -> Vec3 {
    let len_sq = sum_sq3(vec.x, vec.y, vec.z);
    if len_sq <= EPSILON {
        return Vec3::ZERO;
    }
    let inv = rsqrt(len_sq);
    Vec3::new(
        vec.x * inv * length,
        vec.y * inv * length,
        vec.z * inv * length,
    )
}

pub fn vec_cross(a: &Vec3, b: &Vec3) -> Vec3 {
    Vec3::new(
        a.y * b.z - a.z * b.y,
        a.z * b.x - a.x * b.z,
        a.x * b.y - a.y * b.x,
    )
}

pub fn vec_distance(a: &Vec3, b: &Vec3) -> f64 {
    sqrt(sum_sq3(a.x - b.x, a.y - b.y, a.z - b.z))
}

/// `angle_s16` may be a float (callers pass computed angles); it is `ToInt32`'d then halved like `>> 1`.
pub fn quat_from_axis_angle(axis: &Vec3, angle_s16: f64) -> Quat {
    let half = to_int32(angle_s16) >> 1;
    let len_sq = sum_sq3(axis.x, axis.y, axis.z);
    if len_sq < EPSILON {
        return Quat::IDENTITY;
    }
    let inv = rsqrt(len_sq) * sin_s16(half);
    Quat {
        x: axis.x * inv,
        y: axis.y * inv,
        z: axis.z * inv,
        w: cos_s16(half),
    }
}

pub fn quat_normalize(q: &mut Quat) {
    let len_sq = q.x * q.x + q.y * q.y + q.z * q.z + q.w * q.w;
    if len_sq <= EPSILON {
        *q = Quat::IDENTITY;
        return;
    }
    let inv = rsqrt(len_sq);
    q.x *= inv;
    q.y *= inv;
    q.z *= inv;
    q.w *= inv;
}

pub fn quat_mul(a: &Quat, b: &Quat) -> Quat {
    Quat {
        x: a.w * b.x + a.x * b.w + a.y * b.z - a.z * b.y,
        y: a.w * b.y - a.x * b.z + a.y * b.w + a.z * b.x,
        z: a.w * b.z + a.x * b.y - a.y * b.x + a.z * b.w,
        w: a.w * b.w - a.x * b.x - a.y * b.y - a.z * b.z,
    }
}

pub fn quat_from_dirs(from: &Vec3, to: &Vec3) -> Quat {
    let from_len_sq = sum_sq3(from.x, from.y, from.z);
    let to_len_sq = sum_sq3(to.x, to.y, to.z);
    if from_len_sq <= EPSILON || to_len_sq <= EPSILON {
        return Quat::IDENTITY;
    }
    let inv_from = rsqrt(from_len_sq);
    let inv_to = rsqrt(to_len_sq);
    let (fx, fy, fz) = (from.x * inv_from, from.y * inv_from, from.z * inv_from);
    let (tx, ty, tz) = (to.x * inv_to, to.y * inv_to, to.z * inv_to);
    let dot = fx * tx + fy * ty + fz * tz;
    if dot < -0.999999 {
        let (mut ax, mut ay, mut az) = (0.0, 1.0, 0.0);
        if fx.abs() < 0.1 && fz.abs() < 0.1 {
            ax = 1.0;
            ay = 0.0;
            az = 0.0;
        }
        let cx = fy * az - fz * ay;
        let cy = fz * ax - fx * az;
        let cz = fx * ay - fy * ax;
        let axis_len_sq = sum_sq3(cx, cy, cz);
        if axis_len_sq <= EPSILON {
            return Quat::IDENTITY;
        }
        let inv_axis = rsqrt(axis_len_sq);
        return Quat {
            x: cx * inv_axis,
            y: cy * inv_axis,
            z: cz * inv_axis,
            w: 0.0,
        };
    }
    let mut out = Quat {
        x: fy * tz - fz * ty,
        y: fz * tx - fx * tz,
        z: fx * ty - fy * tx,
        w: 1.0 + dot,
    };
    quat_normalize(&mut out);
    out
}

/// Port of `MatrixStack`: a current 3x4 matrix `a`, a spare `b` (`mtxB`) and a push/pop stack.
#[derive(Debug, Clone)]
pub struct MatrixStack {
    pub a: Mtx,
    pub b: Mtx,
    stack: Vec<Mtx>,
}

impl Default for MatrixStack {
    fn default() -> Self {
        Self::new()
    }
}

impl MatrixStack {
    pub fn new() -> Self {
        MatrixStack {
            a: MTX_IDENTITY,
            b: [0.0; 12],
            stack: Vec::new(),
        }
    }

    pub fn from_identity(&mut self) {
        self.a = MTX_IDENTITY;
    }

    pub fn from_mtx(&mut self, src: &Mtx) {
        self.a = *src;
    }

    pub fn to_mtx(&self, dest: &mut Mtx) {
        *dest = self.a;
    }

    pub fn from_translate(&mut self, v: &Vec3) {
        self.from_translate_xyz(v.x, v.y, v.z);
    }

    pub fn from_translate_xyz(&mut self, x: f64, y: f64, z: f64) {
        let (x, y, z) = (x as f32, y as f32, z as f32);
        self.a = [1., 0., 0., x, 0., 1., 0., y, 0., 0., 1., z];
    }

    pub fn from_rotate_x(&mut self, angle: i32) {
        let s = sin_s16(angle) as f32;
        let c = cos_s16(angle) as f32;
        self.a = [1., 0., 0., 0., 0., c, -s, 0., 0., s, c, 0.];
    }

    pub fn from_rotate_y(&mut self, angle: i32) {
        let s = sin_s16(angle) as f32;
        let c = cos_s16(angle) as f32;
        self.a = [c, 0., s, 0., 0., 1., 0., 0., -s, 0., c, 0.];
    }

    pub fn from_rotate_z(&mut self, angle: i32) {
        let s = sin_s16(angle) as f32;
        let c = cos_s16(angle) as f32;
        self.a = [c, -s, 0., 0., s, c, 0., 0., 0., 0., 1., 0.];
    }

    pub fn rotate_x(&mut self, angle: i32) {
        let s = sin_s16(angle);
        let c = cos_s16(angle);
        let m = &mut self.a;
        let (a0, a1) = (m[1] as f64, m[2] as f64);
        let (b0, b1) = (m[5] as f64, m[6] as f64);
        let (c0, c1) = (m[9] as f64, m[10] as f64);
        m[1] = (a0 * c + a1 * s) as f32;
        m[2] = (a0 * -s + a1 * c) as f32;
        m[5] = (b0 * c + b1 * s) as f32;
        m[6] = (b0 * -s + b1 * c) as f32;
        m[9] = (c0 * c + c1 * s) as f32;
        m[10] = (c0 * -s + c1 * c) as f32;
    }

    pub fn rotate_y(&mut self, angle: i32) {
        let s = sin_s16(angle);
        let c = cos_s16(angle);
        let m = &mut self.a;
        let (a0, a1) = (m[0] as f64, m[2] as f64);
        let (b0, b1) = (m[4] as f64, m[6] as f64);
        let (c0, c1) = (m[8] as f64, m[10] as f64);
        m[0] = (a0 * c + a1 * -s) as f32;
        m[2] = (a0 * s + a1 * c) as f32;
        m[4] = (b0 * c + b1 * -s) as f32;
        m[6] = (b0 * s + b1 * c) as f32;
        m[8] = (c0 * c + c1 * -s) as f32;
        m[10] = (c0 * s + c1 * c) as f32;
    }

    pub fn rotate_z(&mut self, angle: i32) {
        let s = sin_s16(angle);
        let c = cos_s16(angle);
        let m = &mut self.a;
        let (a0, a1) = (m[0] as f64, m[1] as f64);
        let (b0, b1) = (m[4] as f64, m[5] as f64);
        let (c0, c1) = (m[8] as f64, m[9] as f64);
        m[0] = (a0 * c + a1 * s) as f32;
        m[1] = (a0 * -s + a1 * c) as f32;
        m[4] = (b0 * c + b1 * s) as f32;
        m[5] = (b0 * -s + b1 * c) as f32;
        m[8] = (c0 * c + c1 * s) as f32;
        m[9] = (c0 * -s + c1 * c) as f32;
    }

    pub fn translate_xyz(&mut self, x: f64, y: f64, z: f64) {
        let m = &mut self.a;
        let tx = m[2] as f64 * z + m[0] as f64 * x + m[3] as f64 + m[1] as f64 * y;
        let ty = m[6] as f64 * z + m[4] as f64 * x + m[7] as f64 + m[5] as f64 * y;
        let tz = m[10] as f64 * z + m[8] as f64 * x + m[11] as f64 + m[9] as f64 * y;
        m[3] = tx as f32;
        m[7] = ty as f32;
        m[11] = tz as f32;
    }

    pub fn translate(&mut self, v: &Vec3) {
        self.translate_xyz(v.x, v.y, v.z);
    }

    pub fn translate_neg(&mut self, v: &Vec3) {
        self.translate_xyz(-v.x, -v.y, -v.z);
    }

    pub fn scale_xyz(&mut self, x: f64, y: f64, z: f64) {
        let m = &mut self.a;
        m[0] = (m[0] as f64 * x) as f32;
        m[1] = (m[1] as f64 * y) as f32;
        m[2] = (m[2] as f64 * z) as f32;
        m[4] = (m[4] as f64 * x) as f32;
        m[5] = (m[5] as f64 * y) as f32;
        m[6] = (m[6] as f64 * z) as f32;
        m[8] = (m[8] as f64 * x) as f32;
        m[9] = (m[9] as f64 * y) as f32;
        m[10] = (m[10] as f64 * z) as f32;
    }

    /// `out = a * b`
    pub fn mult(a: &Mtx, b: &Mtx, out: &mut Mtx) {
        let g = |m: &Mtx, i: usize| m[i] as f64;
        let m00 = g(b, 8) * g(a, 2) + g(b, 4) * g(a, 1) + g(b, 0) * g(a, 0);
        let m01 = g(b, 9) * g(a, 2) + g(b, 5) * g(a, 1) + g(b, 1) * g(a, 0);
        let m02 = g(b, 10) * g(a, 2) + g(b, 6) * g(a, 1) + g(b, 2) * g(a, 0);
        let m03 = g(b, 11) * g(a, 2) + g(b, 7) * g(a, 1) + g(b, 3) * g(a, 0) + g(a, 3);

        let m10 = g(b, 8) * g(a, 6) + g(b, 4) * g(a, 5) + g(b, 0) * g(a, 4);
        let m11 = g(b, 9) * g(a, 6) + g(b, 5) * g(a, 5) + g(b, 1) * g(a, 4);
        let m12 = g(b, 10) * g(a, 6) + g(b, 6) * g(a, 5) + g(b, 2) * g(a, 4);
        let m13 = g(b, 11) * g(a, 6) + g(b, 7) * g(a, 5) + g(b, 3) * g(a, 4) + g(a, 7);

        let m20 = g(b, 8) * g(a, 10) + g(b, 4) * g(a, 9) + g(b, 0) * g(a, 8);
        let m21 = g(b, 9) * g(a, 10) + g(b, 5) * g(a, 9) + g(b, 1) * g(a, 8);
        let m22 = g(b, 10) * g(a, 10) + g(b, 6) * g(a, 9) + g(b, 2) * g(a, 8);
        let m23 = g(b, 11) * g(a, 10) + g(b, 7) * g(a, 9) + g(b, 3) * g(a, 8) + g(a, 11);

        *out = [
            m00 as f32, m01 as f32, m02 as f32, m03 as f32, m10 as f32, m11 as f32, m12 as f32,
            m13 as f32, m20 as f32, m21 as f32, m22 as f32, m23 as f32,
        ];
    }

    /// `current = current * mtx`
    pub fn mult_right(&mut self, mtx: &Mtx) {
        let a = self.a;
        Self::mult(&a, mtx, &mut self.a);
    }

    /// `current = mtx * current`
    pub fn mult_left(&mut self, mtx: &Mtx) {
        let a = self.a;
        Self::mult(mtx, &a, &mut self.a);
    }

    pub fn tf_point(&self, src: &Vec3) -> Vec3 {
        let m = &self.a;
        let (x, y, z) = (src.x, src.y, src.z);
        Vec3::new(
            m[0] as f64 * x + m[1] as f64 * y + m[2] as f64 * z + m[3] as f64,
            m[4] as f64 * x + m[5] as f64 * y + m[6] as f64 * z + m[7] as f64,
            m[8] as f64 * x + m[9] as f64 * y + m[10] as f64 * z + m[11] as f64,
        )
    }

    pub fn tf_vec(&self, src: &Vec3) -> Vec3 {
        let m = &self.a;
        let (x, y, z) = (src.x, src.y, src.z);
        Vec3::new(
            m[0] as f64 * x + m[1] as f64 * y + m[2] as f64 * z,
            m[4] as f64 * x + m[5] as f64 * y + m[6] as f64 * z,
            m[8] as f64 * x + m[9] as f64 * y + m[10] as f64 * z,
        )
    }

    pub fn rigid_inv_tf_point(&self, src: &Vec3) -> Vec3 {
        let m = &self.a;
        let x = src.x - m[3] as f64;
        let y = src.y - m[7] as f64;
        let z = src.z - m[11] as f64;
        Vec3::new(
            m[8] as f64 * z + m[4] as f64 * y + m[0] as f64 * x,
            m[9] as f64 * z + m[5] as f64 * y + m[1] as f64 * x,
            m[10] as f64 * z + m[6] as f64 * y + m[2] as f64 * x,
        )
    }

    pub fn rigid_inv_tf_vec(&self, src: &Vec3) -> Vec3 {
        let m = &self.a;
        let (x, y, z) = (src.x, src.y, src.z);
        Vec3::new(
            m[8] as f64 * z + m[4] as f64 * y + m[0] as f64 * x,
            m[9] as f64 * z + m[5] as f64 * y + m[1] as f64 * x,
            m[10] as f64 * z + m[6] as f64 * y + m[2] as f64 * x,
        )
    }

    pub fn get_translate(&self) -> Vec3 {
        Vec3::new(self.a[3] as f64, self.a[7] as f64, self.a[11] as f64)
    }

    pub fn set_translate(&mut self, v: &Vec3) {
        self.a[3] = v.x as f32;
        self.a[7] = v.y as f32;
        self.a[11] = v.z as f32;
    }

    pub fn normalize_basis(&mut self) {
        let m = &mut self.a;
        let g = |m: &Mtx, i: usize| m[i] as f64;
        let mut inv = rsqrt(g(m, 0) * g(m, 0) + g(m, 4) * g(m, 4) + g(m, 8) * g(m, 8));
        m[0] = (m[0] as f64 * inv) as f32;
        m[4] = (m[4] as f64 * inv) as f32;
        m[8] = (m[8] as f64 * inv) as f32;
        inv = rsqrt(g(m, 1) * g(m, 1) + g(m, 5) * g(m, 5) + g(m, 9) * g(m, 9));
        m[1] = (m[1] as f64 * inv) as f32;
        m[5] = (m[5] as f64 * inv) as f32;
        m[9] = (m[9] as f64 * inv) as f32;
        inv = rsqrt(g(m, 2) * g(m, 2) + g(m, 6) * g(m, 6) + g(m, 10) * g(m, 10));
        m[2] = (m[2] as f64 * inv) as f32;
        m[6] = (m[6] as f64 * inv) as f32;
        m[10] = (m[10] as f64 * inv) as f32;
    }

    pub fn from_quat(&mut self, q: &Quat) {
        let (x, y, z, w) = (q.x, q.y, q.z, q.w);
        let x2 = x + x;
        let y2 = y + y;
        let z2 = z + z;
        let xx = x * x2;
        let yy = y * y2;
        let zz = z * z2;
        let xy = x * y2;
        let xz = x * z2;
        let yz = y * z2;
        let wx = w * x2;
        let wy = w * y2;
        let wz = w * z2;
        let m = &mut self.a;
        m[0] = (1.0 - (yy + zz)) as f32;
        m[5] = (1.0 - (xx + zz)) as f32;
        m[10] = (1.0 - (xx + yy)) as f32;
        m[6] = (yz - wx) as f32;
        m[9] = (yz + wx) as f32;
        m[8] = (xz - wy) as f32;
        m[2] = (xz + wy) as f32;
        m[1] = (xy - wz) as f32;
        m[4] = (xy + wz) as f32;
        m[3] = 0.0;
        m[7] = 0.0;
        m[11] = 0.0;
    }

    pub fn to_quat(&self) -> Quat {
        let m: Vec<f64> = self.a.iter().map(|&v| v as f64).collect();
        let trace = m[0] + m[5] + m[10];
        if trace > 0.0 {
            let s = sqrt(trace + 1.0) * 2.0;
            return Quat {
                w: 0.25 * s,
                x: (m[9] - m[6]) / s,
                y: (m[2] - m[8]) / s,
                z: (m[4] - m[1]) / s,
            };
        }
        if m[0] > m[5] && m[0] > m[10] {
            let s = sqrt(1.0 + m[0] - m[5] - m[10]) * 2.0;
            return Quat {
                w: (m[9] - m[6]) / s,
                x: 0.25 * s,
                y: (m[1] + m[4]) / s,
                z: (m[2] + m[8]) / s,
            };
        }
        if m[5] > m[10] {
            let s = sqrt(1.0 + m[5] - m[0] - m[10]) * 2.0;
            return Quat {
                w: (m[2] - m[8]) / s,
                x: (m[1] + m[4]) / s,
                y: 0.25 * s,
                z: (m[6] + m[9]) / s,
            };
        }
        let s = sqrt(1.0 + m[10] - m[0] - m[5]) * 2.0;
        Quat {
            w: (m[4] - m[1]) / s,
            x: (m[2] + m[8]) / s,
            y: (m[6] + m[9]) / s,
            z: 0.25 * s,
        }
    }

    /// Returns `(rotY, rotX, rotZ)`; `rotY`/`rotZ` are not wrapped (matches the reference).
    pub fn to_euler_yxz(&mut self) -> (i32, i32, i32) {
        let forward = Vec3::new(0.0, 0.0, -1.0);
        let up = Vec3::new(0.0, 1.0, 0.0);
        self.push();
        let forward = self.tf_vec(&forward);
        let up = self.tf_vec(&up);
        let rot_x = atan2_s16(forward.y, sqrt(sum_sq2(forward.x, forward.z)));
        let rot_y = atan2_s16(forward.x, forward.z) - 0x8000;
        self.from_rotate_y(rot_y);
        self.rotate_x(rot_x);
        let up = self.rigid_inv_tf_vec(&up);
        let rot_z = -atan2_s16(up.x, up.y);
        self.pop();
        (rot_y, rot_x, rot_z)
    }

    pub fn push(&mut self) {
        self.stack.push(self.a);
    }

    pub fn pop(&mut self) {
        if let Some(m) = self.stack.pop() {
            self.a = m;
        }
    }
}

/// `cos(t * pi / 2)` for `t = k / 32`, `k = 0..=32` (the banana fly-to-HUD easing), as raw f64 bits. These are
/// plain mathematical values (the correctly rounded cosines, identical to V8's `Math.cos` there); a table keeps
/// the simulation independent of the platform libm.
const COS_QUARTER_STEPS: [u64; 33] = [
    0x3ff0000000000000,
    0x3feff621e3796d7e,
    0x3fefd88da3d12526,
    0x3fefa7557f08a517,
    0x3fef6297cff75cb0,
    0x3fef0a7efb9230d7,
    0x3fee9f4156c62dda,
    0x3fee212104f686e5,
    0x3fed906bcf328d46,
    0x3feced7af43cc773,
    0x3fec38b2f180bdb1,
    0x3feb728345196e3e,
    0x3fea9b66290ea1a3,
    0x3fe9b3e047f38741,
    0x3fe8bc806b151741,
    0x3fe7b5df226aafb0,
    0x3fe6a09e667f3bcd,
    0x3fe57d69348cec9f,
    0x3fe44cf325091dd6,
    0x3fe30ff7fce17036,
    0x3fe1c73b39ae68c9,
    0x3fe073879922ffed,
    0x3fde2b5d3806f63e,
    0x3fdb5d1009e15cc2,
    0x3fd87de2a6aea964,
    0x3fd58f9a75ab1fdd,
    0x3fd294062ed59f05,
    0x3fcf19f97b215f1e,
    0x3fc8f8b83c69a60d,
    0x3fc2c8106e8e613a,
    0x3fb917a6bc29b438,
    0x3fa91f65f10dd824,
    0x3c91a62633145c07,
];

/// `Math.cos((t * Math.PI) / 2)` for `t` a multiple of 1/32 in `[0, 1]`.
pub fn cos_quarter_turn(t: f64) -> f64 {
    let k = (t * 32.0).round();
    if (0.0..=32.0).contains(&k) && k / 32.0 == t {
        f64::from_bits(COS_QUARTER_STEPS[k as usize])
    } else {
        ((t * std::f64::consts::PI) / 2.0).cos()
    }
}

pub fn clamp(value: f64, min: f64, max: f64) -> f64 {
    min.max(value).min(max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quarter_turn_cosine_table_matches_libm() {
        for k in 0..=32 {
            let t = k as f64 / 32.0;
            let want = ((t * std::f64::consts::PI) / 2.0).cos();
            assert!(
                (cos_quarter_turn(t) - want).abs() <= want.abs() * 2e-16 + 1e-300,
                "k = {k}"
            );
        }
        assert_eq!(cos_quarter_turn(0.0), 1.0);
        assert_eq!(
            cos_quarter_turn(0.3),
            (0.3 * std::f64::consts::PI / 2.0).cos()
        );
    }

    #[test]
    fn s16_wrapping() {
        assert_eq!(to_s16(0.0), 0);
        assert_eq!(to_s16(32767.9), 32767);
        assert_eq!(to_s16(32768.0), -32768);
        assert_eq!(to_s16(-32769.0), 32767);
        assert_eq!(to_s16(65536.0 + 5.0), 5);
        assert_eq!(to_s16(-0.9), 0);
        assert_eq!(wrap_s16(0x8000), -0x8000);
        assert_eq!(wrap_s16(-0x8001), 0x7fff);
        assert_eq!(to_int32(4294967296.0 + 3.0), 3);
        assert_eq!(to_int32(-1.5), -1);
        assert_eq!(to_int32(f64::NAN), 0);
    }

    #[test]
    fn trig_quadrants() {
        assert_eq!(sin_s16(0), 0.0);
        assert_eq!(sin_s16(0x4000), 1.0);
        assert_eq!(sin_s16(0x8000), 0.0);
        assert_eq!(sin_s16(0xc000), -1.0);
        assert_eq!(sin_s16(-0x4000), -1.0);
        assert_eq!(cos_s16(0), 1.0);
        assert_eq!(cos_s16(0x8000), -1.0);
        for a in (-0x8000..0x8000).step_by(97) {
            let exact = (a as f64 * std::f64::consts::PI / 32768.0).sin();
            assert!((sin_s16(a) - exact).abs() < 1e-6, "sin {a}");
            let exact = (a as f64 * std::f64::consts::PI / 32768.0).cos();
            assert!((cos_s16(a) - exact).abs() < 1e-6, "cos {a}");
        }
    }

    #[test]
    fn atan2_matches_float_atan2_within_two_units() {
        let k = 32768.0 / std::f64::consts::PI;
        for i in 0..720 {
            let a = i as f64 * std::f64::consts::PI / 360.0;
            let (y, x) = (a.sin() * 3.7, a.cos() * 3.7);
            let got = atan2_s16(y, x);
            let want = to_s16((y.atan2(x) * k).round());
            let d = wrap_s16(got - want).abs();
            assert!(d <= 2, "angle {i}: got {got} want {want}");
        }
        assert_eq!(atan2_s16(0.0, 0.0), 0);
        assert_eq!(atan2_s16(1.0, 0.0), 0x4000);
        assert_eq!(atan2_s16(0.0, -1.0), -0x8000);
        assert_eq!(atan2_s16_safe(1e-9, 1e-9), 0);
    }

    #[test]
    fn custom_sqrt_is_f32_accurate() {
        for v in [0.25, 1.0, 2.0, 3.0, 10.0, 12345.678, 1e-3] {
            let s = sqrt(v);
            assert!((s - v.sqrt()).abs() / v.sqrt() < 2e-7, "sqrt {v}: {s}");
            assert_eq!(s, f32r(s), "result is an f32 value");
            let r = rsqrt(v);
            assert!((r - 1.0 / v.sqrt()).abs() * v.sqrt() < 2e-7);
        }
        assert_eq!(sqrt(0.0), 0.0);
        assert_eq!(sqrt(-4.0), 0.0);
        assert_eq!(rsqrt(0.0), f64::INFINITY);
        assert!(sqrt(f64::NAN).is_nan());
    }

    #[test]
    fn vector_helpers() {
        let mut v = Vec3::new(3.0, 0.0, 4.0);
        let len = vec_normalize_len(&mut v);
        assert!((len - 5.0).abs() < 1e-6);
        assert!((vec_len(&v) - 1.0).abs() < 1e-6);
        let c = vec_cross(&Vec3::new(1.0, 0.0, 0.0), &Vec3::new(0.0, 1.0, 0.0));
        assert_eq!(c, Vec3::new(0.0, 0.0, 1.0));
        assert_eq!(
            vec_dot(&Vec3::new(1.0, 2.0, 3.0), &Vec3::new(4.0, 5.0, 6.0)),
            32.0
        );
        let mut zero = Vec3::ZERO;
        assert_eq!(vec_normalize_len(&mut zero), 0.0);
        assert_eq!(vec_set_len(&Vec3::ZERO, 2.0), Vec3::ZERO);
        let s = vec_set_len(&Vec3::new(0.0, 5.0, 0.0), 2.0);
        assert!((s.y - 2.0).abs() < 1e-6);
        assert!((vec_distance(&Vec3::ZERO, &Vec3::new(0.0, 3.0, 4.0)) - 5.0).abs() < 1e-6);
    }

    #[test]
    fn matrix_stack_rotations_and_inverse() {
        let mut m = MatrixStack::new();
        m.from_translate(&Vec3::new(1.0, 2.0, 3.0));
        m.rotate_y(0x4000);
        // +90 deg about Y maps local +Z to world +X
        let p = m.tf_vec(&Vec3::new(0.0, 0.0, 1.0));
        assert!((p.x - 1.0).abs() < 1e-6 && p.z.abs() < 1e-6, "{p:?}");
        let world = m.tf_point(&Vec3::new(0.0, 0.0, 1.0));
        let back = m.rigid_inv_tf_point(&world);
        assert!((back.z - 1.0).abs() < 1e-6 && back.x.abs() < 1e-6 && back.y.abs() < 1e-6);
        m.push();
        m.rotate_x(0x2000);
        m.pop();
        assert_eq!(m.tf_point(&Vec3::ZERO), Vec3::new(1.0, 2.0, 3.0));
        // translation applies in the rotated frame
        m.translate_xyz(0.0, 0.0, 2.0);
        let t = m.get_translate();
        assert!((t.x - 3.0).abs() < 1e-6 && (t.y - 2.0).abs() < 1e-6 && (t.z - 3.0).abs() < 1e-6);
    }

    #[test]
    fn matrix_mult_matches_sequential_transforms() {
        let mut a = MatrixStack::new();
        a.from_translate(&Vec3::new(1.0, 0.0, 0.0));
        a.rotate_z(0x1000);
        let mut b = MatrixStack::new();
        b.from_translate(&Vec3::new(0.0, 2.0, 0.0));
        b.rotate_x(-0x800);
        let mut out = MTX_IDENTITY;
        MatrixStack::mult(&a.a, &b.a, &mut out);
        let mut c = MatrixStack::new();
        c.from_mtx(&out);
        let p = Vec3::new(0.3, -0.7, 1.1);
        let want = a.tf_point(&b.tf_point(&p));
        let got = c.tf_point(&p);
        assert!(
            (want.x - got.x).abs() < 1e-5
                && (want.y - got.y).abs() < 1e-5
                && (want.z - got.z).abs() < 1e-5
        );
    }

    #[test]
    fn quaternion_roundtrip() {
        let q = quat_from_axis_angle(&Vec3::new(0.0, 1.0, 0.0), 0x4000 as f64);
        let mut m = MatrixStack::new();
        m.from_quat(&q);
        let p = m.tf_vec(&Vec3::new(0.0, 0.0, 1.0));
        assert!((p.x - 1.0).abs() < 1e-5, "{p:?}");
        let back = m.to_quat();
        assert!((back.y - q.y).abs() < 1e-4 && (back.w - q.w).abs() < 1e-4);
        let id = quat_from_dirs(&Vec3::new(0.0, 1.0, 0.0), &Vec3::new(0.0, 1.0, 0.0));
        assert!((id.w - 1.0).abs() < 1e-6);
        let flip = quat_from_dirs(&Vec3::new(0.0, 1.0, 0.0), &Vec3::new(0.0, -1.0, 0.0));
        assert!(flip.w.abs() < 1e-9);
        let prod = quat_mul(&Quat::IDENTITY, &q);
        assert_eq!(prod, q);
    }

    #[test]
    fn euler_extraction_recovers_yaw_pitch() {
        let mut m = MatrixStack::new();
        m.from_rotate_y(0x1000);
        m.rotate_x(0x0800);
        let (ry, rx, rz) = m.to_euler_yxz();
        assert!((wrap_s16(ry) - 0x1000).abs() <= 2, "{ry}");
        assert!((rx - 0x0800).abs() <= 2, "{rx}");
        assert!(rz.abs() <= 2, "{rz}");
    }
}

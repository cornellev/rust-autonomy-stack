//! Sensor mounting poses.
//!
//! # Convention
//!
//! One convention, stated once, so that nothing downstream has to guess.
//!
//! A pose is the rigid transform **from the sensor frame to its parent frame**.
//! For a vector `v` expressed in the sensor frame,
//!
//! ```text
//! v_parent = R * v_sensor + xyz
//! ```
//!
//! so `xyz` is the sensor's origin expressed in the parent frame, in metres, and
//! `R` is an *active* rotation (it moves vectors, it does not re-express axes).
//!
//! `rpy` is **intrinsic Z-Y-X**, also written body 3-2-1: yaw about the body `z`,
//! then pitch about the once-rotated `y`, then roll about the twice-rotated `x`.
//! Composed as extrinsic matrices that is
//!
//! ```text
//! R = Rz(yaw) * Ry(pitch) * Rx(roll)
//! ```
//!
//! with angles in radians. `quat` is `[w, x, y, z]`, unit norm, and denotes the same
//! active rotation; it exists because calibration tools emit quaternions and
//! round-tripping them through Euler angles loses precision and introduces branch
//! ambiguity near gimbal lock. Exactly one of the two is given.
//!
//! Rotation matrices here are row-major: `m[i][j]` is row `i`, column `j`.

/// A resolved sensor pose. Built by [`Registration::extrinsics`], which is where
/// the `rpy`/`quat` exclusivity and unit-norm checks happen, so an `Extrinsics`
/// always holds a valid unit quaternion.
///
/// [`Registration::extrinsics`]: super::Registration::extrinsics
#[derive(Debug, Clone, PartialEq)]
pub struct Extrinsics {
    /// Frame this sensor is mounted in, e.g. `base_link`.
    pub parent: String,
    /// Sensor origin in the parent frame, metres.
    pub xyz: [f64; 3],
    /// Orientation as `[w, x, y, z]`, unit norm.
    pub quat: [f64; 4],
}

impl Extrinsics {
    /// Row-major rotation matrix taking sensor-frame vectors to the parent frame.
    pub fn rotation_matrix(&self) -> [[f64; 3]; 3] {
        quat_to_matrix(self.quat)
    }

    /// Apply the full transform to a point in the sensor frame.
    pub fn transform_point(&self, point: [f64; 3]) -> [f64; 3] {
        let r = self.rotation_matrix();
        let mut out = self.xyz;
        for (i, row) in r.iter().enumerate() {
            out[i] += row[0] * point[0] + row[1] * point[1] + row[2] * point[2];
        }
        out
    }

    /// Recover intrinsic Z-Y-X Euler angles, radians. Ill-conditioned within about
    /// a milliradian of gimbal lock (`|pitch| = pi/2`), where roll and yaw are not
    /// separable; prefer the quaternion there.
    pub fn rpy(&self) -> [f64; 3] {
        matrix_to_rpy(self.rotation_matrix())
    }
}

/// Intrinsic Z-Y-X Euler angles to a unit quaternion `[w, x, y, z]`.
pub fn rpy_to_quat(rpy: [f64; 3]) -> [f64; 4] {
    let [roll, pitch, yaw] = rpy;
    let (sr, cr) = (roll * 0.5).sin_cos();
    let (sp, cp) = (pitch * 0.5).sin_cos();
    let (sy, cy) = (yaw * 0.5).sin_cos();
    [
        cr * cp * cy + sr * sp * sy,
        sr * cp * cy - cr * sp * sy,
        cr * sp * cy + sr * cp * sy,
        cr * cp * sy - sr * sp * cy,
    ]
}

/// Unit quaternion `[w, x, y, z]` to a row-major rotation matrix.
pub fn quat_to_matrix(quat: [f64; 4]) -> [[f64; 3]; 3] {
    let [w, x, y, z] = quat;
    [
        [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y - w * z),
            2.0 * (x * z + w * y),
        ],
        [
            2.0 * (x * y + w * z),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z - w * x),
        ],
        [
            2.0 * (x * z - w * y),
            2.0 * (y * z + w * x),
            1.0 - 2.0 * (x * x + y * y),
        ],
    ]
}

/// Intrinsic Z-Y-X Euler angles to a row-major rotation matrix, built directly as
/// `Rz(yaw) * Ry(pitch) * Rx(roll)`.
///
/// This duplicates `quat_to_matrix(rpy_to_quat(rpy))` on purpose: having both an
/// independent construction and the quaternion route lets a test assert they agree,
/// which is what actually pins the convention against a later refactor.
pub fn rpy_to_matrix(rpy: [f64; 3]) -> [[f64; 3]; 3] {
    let [roll, pitch, yaw] = rpy;
    let (sr, cr) = roll.sin_cos();
    let (sp, cp) = pitch.sin_cos();
    let (sy, cy) = yaw.sin_cos();
    [
        [cy * cp, cy * sp * sr - sy * cr, cy * sp * cr + sy * sr],
        [sy * cp, sy * sp * sr + cy * cr, sy * sp * cr - cy * sr],
        [-sp, cp * sr, cp * cr],
    ]
}

/// Row-major rotation matrix back to intrinsic Z-Y-X Euler angles.
pub fn matrix_to_rpy(m: [[f64; 3]; 3]) -> [f64; 3] {
    let sp = -m[2][0];
    // Clamp before asin: accumulated float error can push this a hair past 1.
    let pitch = sp.clamp(-1.0, 1.0).asin();
    if sp.abs() > 1.0 - 1e-12 {
        // Gimbal lock: roll and yaw are degenerate. Fold the whole rotation into
        // yaw and report zero roll, which is the conventional choice.
        [0.0, pitch, m[0][1].atan2(m[1][1])]
    } else {
        [m[2][1].atan2(m[2][2]), pitch, m[1][0].atan2(m[0][0])]
    }
}

#[cfg(feature = "nalgebra")]
impl From<&Extrinsics> for nalgebra::Isometry3<f64> {
    fn from(extrinsics: &Extrinsics) -> Self {
        let [w, x, y, z] = extrinsics.quat;
        let rotation =
            nalgebra::UnitQuaternion::from_quaternion(nalgebra::Quaternion::new(w, x, y, z));
        let translation =
            nalgebra::Translation3::new(extrinsics.xyz[0], extrinsics.xyz[1], extrinsics.xyz[2]);
        nalgebra::Isometry3::from_parts(translation, rotation)
    }
}

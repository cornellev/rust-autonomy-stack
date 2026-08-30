//! Tests for the sensor catalog and per-car registrations.
//!
//! Two things are being pinned here. First, that a parameter belonging to the
//! wrong hardware kind is a hard error rather than a silently ignored key -- that
//! is the whole reason the kind-specific parameters live behind an adjacently
//! tagged enum, and it is invisible unless a test asserts it. Second, the
//! extrinsics convention, which is a choice rather than a fact and would otherwise
//! be free to drift.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use approx::assert_relative_eq;
use cev_config::sensor::{Camera, Catalog, DistortionModel, SensorModel, SensorSet};
use cev_config::{Car, ConfigError};
use proptest::prelude::*;

/// Just inside a half turn, so the Euler round trip stays on one branch.
const NEAR_PI: f64 = std::f64::consts::PI - 1e-3;

const IMU: &str = r#"
id = "unit-imu"
rate_hz = 400.0
[model]
kind = "imu"
[model.params]
gyro_range = 34.9
accel_range = 156.9
[model.params.noise]
gyro_noise_density = 6.1e-5
gyro_bias_random_walk = 1.0e-6
accel_noise_density = 1.4e-3
accel_bias_random_walk = 4.0e-5
"#;

const CAMERA: &str = r#"
id = "unit-camera"
rate_hz = 60.0
[model]
kind = "camera"
[model.params]
resolution = [1920, 1080]
model = "pinhole"
[model.params.intrinsics]
fx = 1492.3
fy = 1491.8
cx = 954.1
cy = 545.6
[model.params.distortion]
model = "radtan"
coeffs = [-0.0421, 0.0917, 0.00012, -0.00007, -0.0338]
[model.params.noise]
pixel_sigma = 0.4
"#;

static SEQ: AtomicU32 = AtomicU32::new(0);

/// A scratch directory unique to one call. Tests run concurrently and several
/// share a fixture string, so the name must not be derived from the contents.
fn scratch(tag: &str) -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("cev-test-{tag}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Write `text` to a temp file named `<id>.toml` and load it as a catalog entry,
/// so the id-must-match-stem rule is exercised the way it is in production.
fn load_model(id: &str, text: &str) -> Result<SensorModel, ConfigError> {
    let dir = scratch("model");
    let path = dir.join(format!("{id}.toml"));
    std::fs::write(&path, text).unwrap();
    let result = SensorModel::load(&path);
    let _ = std::fs::remove_dir_all(&dir);
    result
}

fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is crates/cev-config.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .to_path_buf()
}

// -- 1. round trips ---------------------------------------------------------

#[test]
fn parses_an_imu_model() {
    let model = load_model("unit-imu", IMU).unwrap();
    assert_eq!(model.kind_name(), "imu");
    let imu = model.as_imu().expect("imu variant");
    assert_relative_eq!(imu.gyro_range, 34.9);
    assert_relative_eq!(imu.noise.gyro_noise_density, 6.1e-5);
}

#[test]
fn parses_a_camera_model() {
    let model = load_model("unit-camera", CAMERA).unwrap();
    assert_eq!(model.kind_name(), "camera");
    let camera = model.as_camera().expect("camera variant");
    assert_eq!(camera.resolution, [1920, 1080]);
    assert_eq!(camera.distortion.model, DistortionModel::Radtan);
    assert_relative_eq!(camera.k_matrix()[0][0], 1492.3);
    assert_relative_eq!(camera.k_matrix()[1][2], 545.6);
}

// -- 2/3. the per-hardware split ------------------------------------------

#[test]
fn rejects_an_unknown_parameter() {
    let text = IMU.replace("gyro_range = 34.9", "gyro_range = 34.9\ngyro_rnage = 34.9");
    let error = load_model("unit-imu", &text).unwrap_err();
    assert!(
        matches!(error, ConfigError::Parse { .. }),
        "typo'd parameter must be a parse error, got {error}"
    );
}

#[test]
fn rejects_a_camera_parameter_on_an_imu() {
    let text = IMU.replace("gyro_range = 34.9", "gyro_range = 34.9\nfx = 1492.3");
    let error = load_model("unit-imu", &text).unwrap_err();
    assert!(
        matches!(error, ConfigError::Parse { .. }),
        "camera intrinsics on an IMU must not parse, got {error}"
    );
}

#[test]
fn rejects_an_imu_parameter_on_a_camera() {
    let text = CAMERA.replace(
        "resolution = [1920, 1080]",
        "resolution = [1920, 1080]\ngyro_range = 34.9",
    );
    let error = load_model("unit-camera", &text).unwrap_err();
    assert!(
        matches!(error, ConfigError::Parse { .. }),
        "gyro range on a camera must not parse, got {error}"
    );
}

// -- 4/5. typed access ------------------------------------------------------

#[test]
fn accessors_are_kind_specific() {
    let imu = load_model("unit-imu", IMU).unwrap();
    assert!(imu.as_imu().is_some());
    assert!(imu.as_camera().is_none());
    assert!(imu.as_lidar().is_none());
}

#[test]
fn a_camera_block_parses_on_its_own() {
    let block = r#"
resolution = [640, 480]
model = "pinhole"
intrinsics = { fx = 500.0, fy = 500.0, cx = 320.0, cy = 240.0 }
noise = { pixel_sigma = 0.5 }
"#;
    let camera = Camera::from_toml_str(block).unwrap();
    assert_eq!(camera.width(), 640);
    assert_eq!(camera.distortion.model, DistortionModel::None);
}

// -- 6/7. camera physics ----------------------------------------------------

#[test]
fn rejects_a_short_distortion_vector() {
    let text = CAMERA.replace(
        "coeffs = [-0.0421, 0.0917, 0.00012, -0.00007, -0.0338]",
        "coeffs = [-0.0421, 0.0917, 0.00012, -0.00007]",
    );
    let error = load_model("unit-camera", &text).unwrap_err();
    let ConfigError::Invalid { message, .. } = &error else {
        panic!("expected Invalid, got {error}");
    };
    assert!(message.contains("5 coefficient"), "{message}");
}

#[test]
fn rejects_a_principal_point_outside_the_image() {
    let text = CAMERA.replace("cx = 954.1", "cx = 2954.1");
    let error = load_model("unit-camera", &text).unwrap_err();
    let ConfigError::Invalid { message, .. } = &error else {
        panic!("expected Invalid, got {error}");
    };
    assert!(message.contains("principal point"), "{message}");
}

#[test]
fn rejects_an_id_that_does_not_match_the_file() {
    let error = load_model("something-else", IMU).unwrap_err();
    assert!(matches!(error, ConfigError::Invalid { .. }), "{error}");
}

// -- 8. registration resolution --------------------------------------------

fn car_with(text: &str) -> (PathBuf, Car) {
    let dir = scratch("car");
    let car_dir = dir.join("testcar");
    std::fs::create_dir_all(&car_dir).unwrap();
    std::fs::write(
        car_dir.join("car.toml"),
        "name = \"testcar\"\n[[nodes]]\nname = \"estimation\"\npackage = \"x\"\n",
    )
    .unwrap();
    std::fs::write(car_dir.join("sensors.toml"), text).unwrap();
    let car = Car::load(&dir, "testcar").unwrap();
    (dir, car)
}

const REG: &str = r#"
[[sensors]]
name = "imu_main"
model = "unit-imu"
parent = "base_link"
xyz = [0.0, 0.0, 0.08]
rpy = [0.0, 0.0, 0.0]
"#;

fn unit_catalog() -> (PathBuf, Catalog) {
    let dir = scratch("catalog");
    std::fs::write(dir.join("unit-imu.toml"), IMU).unwrap();
    let catalog = Catalog::load(&dir).unwrap();
    (dir, catalog)
}

#[test]
fn rejects_an_unknown_model_id() {
    let (dir, car) = car_with(&REG.replace("unit-imu", "no-such-imu"));
    let (cat_dir, catalog) = unit_catalog();
    let error = car.resolve_sensors(&catalog).unwrap_err();
    assert!(
        matches!(error, ConfigError::UnknownModel { .. }),
        "expected UnknownModel, got {error}"
    );
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(cat_dir);
}

#[test]
fn rejects_a_rate_above_the_models_native_rate() {
    let (dir, car) = car_with(&REG.replace(
        "model = \"unit-imu\"",
        "model = \"unit-imu\"\nrate_hz = 800.0",
    ));
    let (cat_dir, catalog) = unit_catalog();
    let error = car.resolve_sensors(&catalog).unwrap_err();
    let ConfigError::Invalid { message, .. } = &error else {
        panic!("expected Invalid, got {error}");
    };
    assert!(message.contains("exceeds"), "{message}");
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(cat_dir);
}

#[test]
fn derives_a_zenoh_key_and_honours_an_override() {
    let (dir, car) = car_with(REG);
    let (cat_dir, catalog) = unit_catalog();
    let sensors = car.resolve_sensors(&catalog).unwrap();
    assert_eq!(sensors[0].key_expr("testcar"), "cev/testcar/imu_main");
    // No override in REG, so the model's native rate shows through.
    assert_eq!(sensors[0].rate_hz(), 400.0);
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&cat_dir);

    // With an override, the car's slower rate wins.
    let (dir, car) = car_with(&REG.replace(
        "model = \"unit-imu\"",
        "model = \"unit-imu\"\nrate_hz = 200.0",
    ));
    let (cat_dir, catalog) = unit_catalog();
    let sensors = car.resolve_sensors(&catalog).unwrap();
    assert_eq!(sensors[0].rate_hz(), 200.0);
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(cat_dir);
}

#[test]
fn rejects_a_duplicate_sensor_name() {
    let text = format!("{REG}{REG}");
    let error = SensorSet::load(Path::new("/nonexistent")).unwrap();
    assert!(error.sensors.is_empty(), "missing file is not an error");

    let dir = scratch("dup");
    std::fs::write(dir.join("sensors.toml"), &text).unwrap();
    let error = SensorSet::load(&dir).unwrap_err();
    let ConfigError::Invalid { message, .. } = &error else {
        panic!("expected Invalid, got {error}");
    };
    assert!(message.contains("duplicate"), "{message}");
    let _ = std::fs::remove_dir_all(dir);
}

// -- 9. rotation representation exclusivity --------------------------------

fn extrinsics_error(patch: &str) -> String {
    let dir = scratch("extrinsics");
    std::fs::write(dir.join("sensors.toml"), patch).unwrap();
    let error = SensorSet::load(&dir).unwrap_err();
    let _ = std::fs::remove_dir_all(dir);
    error.to_string()
}

#[test]
fn requires_exactly_one_rotation_representation() {
    let both = REG.replace(
        "rpy = [0.0, 0.0, 0.0]",
        "rpy = [0.0, 0.0, 0.0]\nquat = [1.0, 0.0, 0.0, 0.0]",
    );
    assert!(extrinsics_error(&both).contains("both"), "both set");

    let neither = REG.replace("rpy = [0.0, 0.0, 0.0]\n", "");
    assert!(
        extrinsics_error(&neither).contains("neither"),
        "neither set"
    );

    let unnormalized = REG.replace("rpy = [0.0, 0.0, 0.0]", "quat = [0.5, 0.0, 0.0, 0.0]");
    assert!(
        extrinsics_error(&unnormalized).contains("unit-norm"),
        "unnormalized quaternion"
    );
}

// -- 10/11/13. the extrinsics convention -----------------------------------

fn transpose_times(m: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let mut out = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            out[i][j] = (0..3).map(|k| m[k][i] * m[k][j]).sum();
        }
    }
    out
}

fn det(m: [[f64; 3]; 3]) -> f64 {
    m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
}

#[test]
fn yaw_of_pi_flips_x() {
    // Pins the sign and axis order of the ZYX convention. If someone later swaps
    // to XYZ or to a passive rotation, this is what catches it.
    let r = cev_config::sensor::rpy_to_matrix([0.0, 0.0, std::f64::consts::PI]);
    assert_relative_eq!(r[0][0], -1.0, epsilon = 1e-12);
    assert_relative_eq!(r[1][1], -1.0, epsilon = 1e-12);
    assert_relative_eq!(r[2][2], 1.0, epsilon = 1e-12);
}

#[test]
fn pitch_maps_x_downwards() {
    // Positive pitch about y takes +x toward -z, the aerospace sign convention.
    let r = cev_config::sensor::rpy_to_matrix([0.0, 0.5, 0.0]);
    assert_relative_eq!(r[2][0], -0.5f64.sin(), epsilon = 1e-12);
}

proptest! {
    #[test]
    fn rotation_is_a_proper_rotation(
        roll in -NEAR_PI..NEAR_PI,
        pitch in -1.5f64..1.5f64,
        yaw in -NEAR_PI..NEAR_PI,
    ) {
        let r = cev_config::sensor::rpy_to_matrix([roll, pitch, yaw]);
        let rtr = transpose_times(r);
        for (i, row) in rtr.iter().enumerate() {
            for (j, value) in row.iter().enumerate() {
                let expected = if i == j { 1.0 } else { 0.0 };
                prop_assert!((value - expected).abs() < 1e-10, "R^T R not identity");
            }
        }
        prop_assert!((det(r) - 1.0).abs() < 1e-10, "det(R) != +1");
    }

    /// The quaternion route and the direct matrix construction must agree. This is
    /// why both exist -- either alone could be quietly wrong.
    #[test]
    fn quaternion_and_matrix_routes_agree(
        roll in -NEAR_PI..NEAR_PI,
        pitch in -1.5f64..1.5f64,
        yaw in -NEAR_PI..NEAR_PI,
    ) {
        let direct = cev_config::sensor::rpy_to_matrix([roll, pitch, yaw]);
        let via_quat = cev_config::sensor::quat_to_matrix(
            cev_config::sensor::rpy_to_quat([roll, pitch, yaw]),
        );
        for (i, (direct_row, quat_row)) in direct.iter().zip(via_quat.iter()).enumerate() {
            for (j, (a, b)) in direct_row.iter().zip(quat_row.iter()).enumerate() {
                prop_assert!(
                    (a - b).abs() < 1e-10,
                    "rpy->R and rpy->q->R disagree at ({i}, {j})"
                );
            }
        }
    }

    /// Away from gimbal lock, angles survive the round trip.
    #[test]
    fn euler_angles_round_trip(
        roll in -3.0f64..3.0,
        pitch in -1.4f64..1.4,
        yaw in -3.0f64..3.0,
    ) {
        let recovered = cev_config::sensor::matrix_to_rpy(
            cev_config::sensor::rpy_to_matrix([roll, pitch, yaw]),
        );
        prop_assert!((recovered[0] - roll).abs() < 1e-9);
        prop_assert!((recovered[1] - pitch).abs() < 1e-9);
        prop_assert!((recovered[2] - yaw).abs() < 1e-9);
    }
}

// -- 12. the real catalog ---------------------------------------------------

#[test]
fn the_repo_catalog_loads_and_validates() {
    let catalog = Catalog::load(&repo_root().join("sensors")).unwrap();
    assert!(
        !catalog.is_empty(),
        "the seeded catalog should not be empty"
    );
    for model in catalog.models() {
        assert!(
            model.rate_hz.is_finite() && model.rate_hz > 0.0,
            "{}: bad rate",
            model.id
        );
        if let Some(imu) = model.as_imu() {
            let sampled = imu.noise.sampled(model.rate_hz);
            // sigma_d = sigma_c * sqrt(f); check the relation holds rather than
            // just that the number is finite.
            assert_relative_eq!(
                sampled.gyro_sigma,
                imu.noise.gyro_noise_density * model.rate_hz.sqrt(),
                epsilon = 1e-15
            );
        }
    }
}

#[test]
fn every_car_resolves_against_the_repo_catalog() {
    let root = repo_root();
    let cars_dir = root.join("cars");
    let catalog = Catalog::load(&root.join("sensors")).unwrap();
    for name in Car::list(&cars_dir).unwrap() {
        let car = Car::load(&cars_dir, &name).unwrap();
        let sensors = car
            .resolve_sensors(&catalog)
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        for sensor in sensors {
            sensor
                .extrinsics()
                .unwrap_or_else(|error| panic!("{name}: {error}"));
        }
    }
}

// -- the optional nalgebra bridge ------------------------------------------

/// Only compiled with `--features nalgebra`. Quaternion storage order is the
/// classic place this conversion goes wrong -- nalgebra's `Quaternion::new` takes
/// `w` first while its internal `coords` is `[x, y, z, w]` -- so assert against
/// our own matrix rather than trusting the constructor.
#[cfg(feature = "nalgebra")]
#[test]
fn isometry_matches_our_own_rotation() {
    use cev_config::sensor::{Extrinsics, rpy_to_quat};

    let rpy = [0.3, -0.2, 1.1];
    let extrinsics = Extrinsics {
        parent: "base_link".to_string(),
        xyz: [1.0, 2.0, 3.0],
        quat: rpy_to_quat(rpy),
    };
    let isometry: nalgebra::Isometry3<f64> = (&extrinsics).into();
    let ours = extrinsics.rotation_matrix();
    let theirs = isometry.rotation.to_rotation_matrix();
    for (i, row) in ours.iter().enumerate() {
        for (j, value) in row.iter().enumerate() {
            assert_relative_eq!(*value, theirs[(i, j)], epsilon = 1e-12);
        }
    }
    let point = isometry.transform_point(&nalgebra::Point3::new(0.4, -0.1, 0.7));
    let mine = extrinsics.transform_point([0.4, -0.1, 0.7]);
    for (a, b) in mine.iter().zip(point.coords.iter()) {
        assert_relative_eq!(*a, *b, epsilon = 1e-12);
    }
}

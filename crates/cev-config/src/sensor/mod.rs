//! The sensor registry: a car-agnostic catalog, and per-car registrations of it.
//!
//! Two files, two jobs.
//!
//! `sensors/<id>.toml` is the **catalog**: one entry per sensor model the team owns,
//! independent of any vehicle. An entry is common metadata plus exactly one
//! kind-specific parameter block, because the parameters of a camera and of an IMU
//! have nothing in common -- intrinsics and distortion mean nothing to a gyro, and
//! bias random walk means nothing to an image sensor. See [`Kind`].
//!
//! The catalog exists for **sim-real fidelity**. Its noise parameters, measured by
//! Allan variance or FFT analysis of bench data, are read twice: the simulator
//! corrupts ground truth with them, and the estimator builds its measurement
//! covariance `R` from them. They are one number so the two cannot drift apart.
//!
//! `cars/<car>/sensors.toml` is the **registration**: which catalog models this
//! vehicle actually carries, under what instance names, and where they are mounted.
//! Pose belongs here and not in the catalog -- extrinsics are a property of the
//! mounting, not of the part number.
//!
//! Joining the two is an explicit step, [`Car::resolve_sensors`], which yields
//! [`Sensor`] values pairing a registration with its model.
//!
//! [`Car::resolve_sensors`]: crate::Car::resolve_sensors

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::ConfigError;

mod camera;
mod encoder;
mod extrinsics;
mod gnss;
mod imu;
mod lidar;

pub use camera::{
    Camera, CameraNoise, Distortion, DistortionModel, Intrinsics, Projection, Shutter,
};
pub use encoder::{EncoderNoise, WheelEncoder};
pub use extrinsics::{Extrinsics, matrix_to_rpy, quat_to_matrix, rpy_to_matrix, rpy_to_quat};
pub use gnss::{Gnss, GnssFixNoise, GnssNoise};
pub use imu::{Imu, ImuNoise, ImuSampled};
pub use lidar::{Lidar, LidarNoise};

/// One sensor model in the catalog: a part number, not a physical unit.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SensorModel {
    /// Must match the file stem under `sensors/`.
    pub id: String,
    #[serde(default)]
    pub vendor: String,
    #[serde(default)]
    pub part_number: String,
    #[serde(default)]
    pub description: String,
    /// Native output rate. A car may register a slower rate, never a faster one.
    pub rate_hz: f64,
    /// The kind-specific parameter block.
    pub model: Kind,
    /// Where the numbers above came from.
    pub calibration: Option<Calibration>,
    /// Forward-looking: sizes the zenoh shared-memory provider once the transport
    /// lands. Nothing reads it yet.
    pub shm: Option<Shm>,
}

/// The hardware-specific half of a catalog entry.
///
/// Adjacently tagged (`kind` + `params`) rather than flattened or internally tagged:
/// serde silently disables `deny_unknown_fields` under `#[serde(flatten)]`, and
/// internal tagging reports the tag itself as an unknown field. Adjacent tagging
/// composes, so a parameter belonging to the wrong hardware -- `fx` on an IMU,
/// `gyro_range` on a camera -- is a hard parse error rather than a silently dropped
/// key that leaves a physical constant at its default.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", content = "params", rename_all = "snake_case")]
pub enum Kind {
    Imu(Imu),
    Camera(Camera),
    Gnss(Gnss),
    Lidar(Lidar),
    WheelEncoder(WheelEncoder),
}

/// Provenance for the noise numbers. `Datasheet` means *not measured*: a datasheet
/// sigma quietly entering an estimator's `R` is the classic sim-real trap, so
/// `cev check` counts these and `cev sensors` marks them.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Calibration {
    pub method: Method,
    /// ISO-8601 date the bench data was taken.
    #[serde(default)]
    pub measured_on: String,
    /// Where the raw run lives, so a number can be traced back to its data.
    #[serde(default)]
    pub dataset: String,
    #[serde(default)]
    pub notes: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Method {
    /// Allan variance of a long static run.
    Allan,
    /// Power spectral density / FFT of bench data.
    Fft,
    /// Copied from the vendor. Not measured.
    Datasheet,
}

/// Shared-memory sizing hints for the eventual zenoh SHM provider.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Shm {
    /// Upper bound on one serialized frame.
    pub frame_bytes: usize,
    /// Ring depth. A slow subscriber may fall this many frames behind.
    #[serde(default = "default_slots")]
    pub slots: usize,
}

fn default_slots() -> usize {
    16
}

impl SensorModel {
    pub fn as_imu(&self) -> Option<&Imu> {
        match &self.model {
            Kind::Imu(imu) => Some(imu),
            _ => None,
        }
    }
    pub fn as_camera(&self) -> Option<&Camera> {
        match &self.model {
            Kind::Camera(camera) => Some(camera),
            _ => None,
        }
    }
    pub fn as_gnss(&self) -> Option<&Gnss> {
        match &self.model {
            Kind::Gnss(gnss) => Some(gnss),
            _ => None,
        }
    }
    pub fn as_lidar(&self) -> Option<&Lidar> {
        match &self.model {
            Kind::Lidar(lidar) => Some(lidar),
            _ => None,
        }
    }
    pub fn as_wheel_encoder(&self) -> Option<&WheelEncoder> {
        match &self.model {
            Kind::WheelEncoder(encoder) => Some(encoder),
            _ => None,
        }
    }

    /// Lower-case name of the hardware kind, for display.
    pub fn kind_name(&self) -> &'static str {
        match &self.model {
            Kind::Imu(_) => "imu",
            Kind::Camera(_) => "camera",
            Kind::Gnss(_) => "gnss",
            Kind::Lidar(_) => "lidar",
            Kind::WheelEncoder(_) => "wheel_encoder",
        }
    }

    /// True when the noise numbers came from a vendor sheet rather than measurement.
    pub fn is_datasheet(&self) -> bool {
        matches!(
            self.calibration.as_ref().map(|c| c.method),
            Some(Method::Datasheet) | None
        )
    }

    /// Parse one catalog file. `id` must match the file stem.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let model: SensorModel = toml::from_str(&text).map_err(|source| ConfigError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
        model.validate(path)?;
        Ok(model)
    }

    fn validate(&self, path: &Path) -> Result<(), ConfigError> {
        let invalid = |message: String| ConfigError::Invalid {
            path: path.to_path_buf(),
            message,
        };

        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        if self.id != stem {
            return Err(invalid(format!(
                "id = \"{}\" does not match file stem `{stem}`",
                self.id
            )));
        }
        if !self.rate_hz.is_finite() || self.rate_hz <= 0.0 {
            return Err(invalid(format!(
                "`{}`: rate_hz must be finite and positive, got {}",
                self.id, self.rate_hz
            )));
        }
        if let Some(shm) = &self.shm
            && (shm.frame_bytes == 0 || shm.slots == 0)
        {
            return Err(invalid(format!(
                "`{}`: shm.frame_bytes and shm.slots must be non-zero",
                self.id
            )));
        }
        self.model.validate(&self.id, path)
    }
}

impl Kind {
    fn validate(&self, id: &str, path: &Path) -> Result<(), ConfigError> {
        match self {
            Kind::Imu(imu) => imu.validate(id, path),
            Kind::Camera(camera) => camera.validate(id, path),
            Kind::Gnss(gnss) => gnss.validate(id, path),
            Kind::Lidar(lidar) => lidar.validate(id, path),
            Kind::WheelEncoder(encoder) => encoder.validate(id, path),
        }
    }
}

/// Helper shared by the per-kind `validate` implementations.
pub(crate) fn require(
    ok: bool,
    id: &str,
    path: &Path,
    message: impl FnOnce() -> String,
) -> Result<(), ConfigError> {
    if ok {
        Ok(())
    } else {
        Err(ConfigError::Invalid {
            path: path.to_path_buf(),
            message: format!("`{id}`: {}", message()),
        })
    }
}

/// Every sensor model under `sensors/`, keyed by id.
#[derive(Debug, Clone, Default)]
pub struct Catalog {
    models: BTreeMap<String, SensorModel>,
}

impl Catalog {
    /// Load every `*.toml` under `dir`. A missing directory yields an empty catalog,
    /// so a group that has not defined any sensors yet still works.
    pub fn load(dir: &Path) -> Result<Self, ConfigError> {
        let mut models = BTreeMap::new();
        if !dir.is_dir() {
            return Ok(Self { models });
        }
        let entries = std::fs::read_dir(dir).map_err(|source| ConfigError::Io {
            path: dir.to_path_buf(),
            source,
        })?;
        for entry in entries {
            let entry = entry.map_err(|source| ConfigError::Io {
                path: dir.to_path_buf(),
                source,
            })?;
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "toml") {
                let model = SensorModel::load(&path)?;
                models.insert(model.id.clone(), model);
            }
        }
        Ok(Self { models })
    }

    pub fn get(&self, id: &str) -> Option<&SensorModel> {
        self.models.get(id)
    }

    /// Every model, sorted by id.
    pub fn models(&self) -> impl Iterator<Item = &SensorModel> {
        self.models.values()
    }

    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.models.keys().map(String::as_str)
    }

    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }

    pub fn len(&self) -> usize {
        self.models.len()
    }
}

/// One sensor this car carries: a catalog model, named and mounted.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Registration {
    /// Instance name, unique within the car. Becomes the last chunk of the zenoh key.
    pub name: String,
    /// Catalog id under `sensors/`.
    pub model: String,
    #[serde(default = "crate::default_true")]
    pub enabled: bool,
    /// Run slower than the model's native rate. Never faster.
    pub rate_hz: Option<f64>,
    /// Explicit zenoh key expression. Defaults to `cev/<car>/<name>`.
    pub key: Option<String>,
    /// Frame this sensor is mounted in, e.g. `base_link`.
    pub parent: String,
    /// Translation from `parent` to the sensor, in metres.
    pub xyz: [f64; 3],
    /// Orientation as intrinsic Z-Y-X Euler angles in radians. See [`Extrinsics`].
    /// Exactly one of `rpy` or `quat`.
    pub rpy: Option<[f64; 3]>,
    /// Orientation as `[w, x, y, z]`, for poses emitted by a calibration tool.
    pub quat: Option<[f64; 4]>,
}

/// The `[[sensors]]` list in `cars/<car>/sensors.toml`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SensorSet {
    #[serde(default)]
    pub sensors: Vec<Registration>,
}

impl SensorSet {
    /// Load `<car_dir>/sensors.toml`. Absent is not an error -- a car may carry none.
    pub fn load(car_dir: &Path) -> Result<Self, ConfigError> {
        let path = car_dir.join("sensors.toml");
        if !path.is_file() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(&path).map_err(|source| ConfigError::Io {
            path: path.clone(),
            source,
        })?;
        let set: SensorSet = toml::from_str(&text).map_err(|source| ConfigError::Parse {
            path: path.clone(),
            source,
        })?;
        set.validate(&path)?;
        Ok(set)
    }

    fn validate(&self, path: &Path) -> Result<(), ConfigError> {
        let invalid = |message: String| ConfigError::Invalid {
            path: path.to_path_buf(),
            message,
        };

        let mut seen = BTreeMap::new();
        for sensor in &self.sensors {
            if seen.insert(sensor.name.clone(), ()).is_some() {
                return Err(invalid(format!("duplicate sensor `{}`", sensor.name)));
            }
            // The name becomes a zenoh key chunk, so it may not contain the
            // separator or any of the wildcard/verbatim sigils.
            if sensor.name.is_empty() || sensor.name.contains(['/', '*', '?', '#', '$', ' ']) {
                return Err(invalid(format!(
                    "sensor `{}`: name must be a single zenoh key chunk \
                     (no `/ * ? # $` or spaces)",
                    sensor.name
                )));
            }
            if let Some(rate) = sensor.rate_hz
                && (!rate.is_finite() || rate <= 0.0)
            {
                return Err(invalid(format!(
                    "sensor `{}`: rate_hz must be finite and positive, got {rate}",
                    sensor.name
                )));
            }
            if !sensor.xyz.iter().all(|v| v.is_finite()) {
                return Err(invalid(format!(
                    "sensor `{}`: xyz must be finite",
                    sensor.name
                )));
            }
            sensor.extrinsics_checked(path)?;
        }
        Ok(())
    }
}

impl Registration {
    /// Resolved pose, after checking exactly one rotation representation is given.
    pub fn extrinsics(&self) -> Result<Extrinsics, ConfigError> {
        self.extrinsics_checked(Path::new("sensors.toml"))
    }

    fn extrinsics_checked(&self, path: &Path) -> Result<Extrinsics, ConfigError> {
        let invalid = |message: String| ConfigError::Invalid {
            path: path.to_path_buf(),
            message,
        };
        let quat = match (self.rpy, self.quat) {
            (Some(_), Some(_)) => {
                return Err(invalid(format!(
                    "sensor `{}` sets both `rpy` and `quat`; pick one",
                    self.name
                )));
            }
            (None, None) => {
                return Err(invalid(format!(
                    "sensor `{}` sets neither `rpy` nor `quat`",
                    self.name
                )));
            }
            (Some(rpy), None) => {
                if !rpy.iter().all(|v| v.is_finite()) {
                    return Err(invalid(format!(
                        "sensor `{}`: rpy must be finite",
                        self.name
                    )));
                }
                rpy_to_quat(rpy)
            }
            (None, Some(quat)) => {
                let norm = quat.iter().map(|v| v * v).sum::<f64>().sqrt();
                if !norm.is_finite() || (norm - 1.0).abs() > 1e-6 {
                    return Err(invalid(format!(
                        "sensor `{}`: quat must be unit-norm ([w, x, y, z]), \
                         got norm {norm}",
                        self.name
                    )));
                }
                quat
            }
        };
        Ok(Extrinsics {
            parent: self.parent.clone(),
            xyz: self.xyz,
            quat,
        })
    }
}

/// A registration joined to its catalog model.
#[derive(Debug, Clone, Copy)]
pub struct Sensor<'a> {
    pub reg: &'a Registration,
    pub model: &'a SensorModel,
}

impl<'a> Sensor<'a> {
    /// Effective rate: the car's override if it set one, else the model's native rate.
    pub fn rate_hz(&self) -> f64 {
        self.reg.rate_hz.unwrap_or(self.model.rate_hz)
    }

    /// Zenoh key expression. Derived as `cev/<car>/<name>` unless overridden, so
    /// name-uniqueness within a car is enough to guarantee global uniqueness.
    pub fn key_expr(&self, car: &str) -> String {
        match &self.reg.key {
            Some(key) => key.clone(),
            None => format!("cev/{car}/{}", self.reg.name),
        }
    }

    pub fn extrinsics(&self) -> Result<Extrinsics, ConfigError> {
        self.reg.extrinsics()
    }
}

/// Error path for a registration naming a model the catalog does not have.
pub(crate) fn unknown_model(path: PathBuf, sensor: &str, model: &str) -> ConfigError {
    ConfigError::UnknownModel {
        path,
        sensor: sensor.to_string(),
        model: model.to_string(),
    }
}

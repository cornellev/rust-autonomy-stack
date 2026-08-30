//! Inertial measurement units.

use std::path::Path;

use serde::Deserialize;

use super::require;
use crate::ConfigError;

/// An IMU's physical parameters.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Imu {
    /// Full-scale gyro range, rad/s.
    pub gyro_range: f64,
    /// Full-scale accelerometer range, m/s^2.
    pub accel_range: f64,
    /// Scale-factor error, parts per million.
    #[serde(default)]
    pub scale_factor_ppm: f64,
    /// Worst-case axis misalignment, radians.
    #[serde(default)]
    pub misalignment_rad: f64,
    pub noise: ImuNoise,
}

/// Continuous-time noise parameters, as they come out of an Allan variance plot.
///
/// These are *densities*, not per-sample standard deviations. For a density
/// `sigma_c` sampled at `f` Hz the per-sample stddev is
///
/// ```text
/// sigma_d = sigma_c * sqrt(f)
/// ```
///
/// and a bias driven by random walk `K` drifts over `dt` by `sigma_b = K * sqrt(dt)`.
///
/// The two consumers want different forms and must not each re-derive the square
/// root: a simulator corrupting ground truth per sample wants `sigma_d`, while a
/// continuous-time filter's process noise `Q` takes `sigma_c^2` directly. Call
/// [`ImuNoise::sampled`] for the former rather than writing the conversion again.
///
/// `bias_instability` is the Allan deviation floor. It is reported here because it
/// is what the plot actually shows, but note it is *not* directly a `Q` term -- a
/// first-order Gauss-Markov bias state is driven by `bias_random_walk`.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImuNoise {
    /// Angle random walk N, rad/s/sqrt(Hz).
    pub gyro_noise_density: f64,
    /// Rate random walk K, rad/s^2/sqrt(Hz).
    pub gyro_bias_random_walk: f64,
    /// Allan deviation floor B, rad/s.
    #[serde(default)]
    pub gyro_bias_instability: f64,
    /// Velocity random walk, m/s^2/sqrt(Hz).
    pub accel_noise_density: f64,
    /// Accel bias random walk, m/s^3/sqrt(Hz).
    pub accel_bias_random_walk: f64,
    /// Allan deviation floor, m/s^2.
    #[serde(default)]
    pub accel_bias_instability: f64,
}

/// Per-sample standard deviations at a given rate. See [`ImuNoise`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImuSampled {
    /// rad/s
    pub gyro_sigma: f64,
    /// m/s^2
    pub accel_sigma: f64,
    /// rad/s, bias step over one sample interval
    pub gyro_bias_sigma: f64,
    /// m/s^2, bias step over one sample interval
    pub accel_bias_sigma: f64,
}

impl ImuNoise {
    /// Discretize to `rate_hz`. The single place `sqrt(f)` is written.
    pub fn sampled(&self, rate_hz: f64) -> ImuSampled {
        let root_rate = rate_hz.sqrt();
        let dt = 1.0 / rate_hz;
        let root_dt = dt.sqrt();
        ImuSampled {
            gyro_sigma: self.gyro_noise_density * root_rate,
            accel_sigma: self.accel_noise_density * root_rate,
            gyro_bias_sigma: self.gyro_bias_random_walk * root_dt,
            accel_bias_sigma: self.accel_bias_random_walk * root_dt,
        }
    }

    pub(crate) fn terms(&self) -> [(&'static str, f64); 6] {
        [
            ("gyro_noise_density", self.gyro_noise_density),
            ("gyro_bias_random_walk", self.gyro_bias_random_walk),
            ("gyro_bias_instability", self.gyro_bias_instability),
            ("accel_noise_density", self.accel_noise_density),
            ("accel_bias_random_walk", self.accel_bias_random_walk),
            ("accel_bias_instability", self.accel_bias_instability),
        ]
    }
}

impl Imu {
    pub fn from_toml_str(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }

    pub(crate) fn validate(&self, id: &str, path: &Path) -> Result<(), ConfigError> {
        require(self.gyro_range > 0.0, id, path, || {
            format!("gyro_range must be positive, got {}", self.gyro_range)
        })?;
        require(self.accel_range > 0.0, id, path, || {
            format!("accel_range must be positive, got {}", self.accel_range)
        })?;
        for (name, value) in self.noise.terms() {
            require(value.is_finite() && value >= 0.0, id, path, || {
                format!("{name} must be finite and non-negative, got {value}")
            })?;
        }
        // A gyro whose white noise alone saturates the range is a units mistake,
        // almost always deg vs rad.
        require(
            self.noise.gyro_noise_density < self.gyro_range,
            id,
            path,
            || {
                format!(
                    "gyro_noise_density ({}) exceeds gyro_range ({}); check deg vs rad",
                    self.noise.gyro_noise_density, self.gyro_range
                )
            },
        )?;
        Ok(())
    }
}

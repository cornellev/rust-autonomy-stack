//! Spinning and solid-state lidars.

use std::path::Path;

use serde::Deserialize;

use super::require;
use crate::ConfigError;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lidar {
    /// Beams in the vertical fan.
    pub channels: u32,
    pub horizontal_fov_deg: f64,
    pub vertical_fov_deg: f64,
    pub min_range_m: f64,
    pub max_range_m: f64,
    /// Points in one full sweep, across all channels.
    pub points_per_rev: u32,
    pub noise: LidarNoise,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LidarNoise {
    /// Range sigma at close range, metres.
    pub range_sigma_m: f64,
    /// Range-proportional term, as a fraction: total sigma is
    /// `range_sigma_m + range_sigma_rel * range`.
    #[serde(default)]
    pub range_sigma_rel: f64,
    /// Standard deviation of a point's timestamp, seconds.
    #[serde(default)]
    pub timestamp_jitter_s: f64,
    /// Fraction of returns lost, in `[0, 1]`.
    #[serde(default)]
    pub dropout_rate: f64,
}

impl LidarNoise {
    /// Range sigma at a given range, metres.
    pub fn sigma_at(&self, range_m: f64) -> f64 {
        self.range_sigma_m + self.range_sigma_rel * range_m
    }
}

impl Lidar {
    pub fn from_toml_str(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }

    pub(crate) fn validate(&self, id: &str, path: &Path) -> Result<(), ConfigError> {
        require(self.channels > 0, id, path, || {
            "channels must be non-zero".to_string()
        })?;
        require(self.points_per_rev > 0, id, path, || {
            "points_per_rev must be non-zero".to_string()
        })?;
        for (name, fov) in [
            ("horizontal_fov_deg", self.horizontal_fov_deg),
            ("vertical_fov_deg", self.vertical_fov_deg),
        ] {
            require(
                fov.is_finite() && fov > 0.0 && fov <= 360.0,
                id,
                path,
                || format!("{name} must lie in (0, 360], got {fov}"),
            )?;
        }
        require(
            self.min_range_m.is_finite()
                && self.max_range_m.is_finite()
                && self.min_range_m >= 0.0
                && self.min_range_m < self.max_range_m,
            id,
            path,
            || {
                format!(
                    "expected 0 <= min_range_m < max_range_m, got {} and {}",
                    self.min_range_m, self.max_range_m
                )
            },
        )?;
        for (name, value) in [
            ("range_sigma_m", self.noise.range_sigma_m),
            ("range_sigma_rel", self.noise.range_sigma_rel),
            ("timestamp_jitter_s", self.noise.timestamp_jitter_s),
        ] {
            require(value.is_finite() && value >= 0.0, id, path, || {
                format!("noise.{name} must be finite and non-negative, got {value}")
            })?;
        }
        require(
            (0.0..=1.0).contains(&self.noise.dropout_rate),
            id,
            path,
            || {
                format!(
                    "noise.dropout_rate must lie in [0, 1], got {}",
                    self.noise.dropout_rate
                )
            },
        )?;
        Ok(())
    }
}

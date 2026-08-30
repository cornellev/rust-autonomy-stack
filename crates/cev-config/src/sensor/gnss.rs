//! GNSS receivers.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

use super::require;
use crate::ConfigError;

/// Fix qualities a receiver can report. Their position sigmas differ by two orders
/// of magnitude, so a single scalar would misstate the covariance most of the time.
pub const FIX_TYPES: [&str; 5] = ["none", "single", "dgps", "float", "fixed"];

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Gnss {
    /// e.g. `["gps", "glonass", "galileo"]`.
    #[serde(default)]
    pub constellations: Vec<String>,
    /// Antenna phase centre relative to the receiver's mounting point, metres.
    #[serde(default)]
    pub antenna_offset_m: [f64; 3],
    /// Whether this receiver can produce RTK fixes.
    #[serde(default)]
    pub rtk: bool,
    pub noise: GnssNoise,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GnssNoise {
    /// Fallback horizontal position sigma, metres.
    pub horizontal_sigma_m: f64,
    /// Fallback vertical position sigma, metres.
    pub vertical_sigma_m: f64,
    /// Velocity sigma, m/s.
    pub velocity_sigma_mps: f64,
    /// Per-fix-type override. Keys must come from [`FIX_TYPES`].
    #[serde(default)]
    pub by_fix: BTreeMap<String, GnssFixNoise>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GnssFixNoise {
    pub horizontal_sigma_m: f64,
    pub vertical_sigma_m: f64,
    pub velocity_sigma_mps: f64,
}

impl GnssNoise {
    /// Sigmas for a reported fix type, falling back to the scalar values.
    pub fn for_fix(&self, fix: &str) -> GnssFixNoise {
        self.by_fix.get(fix).copied().unwrap_or(GnssFixNoise {
            horizontal_sigma_m: self.horizontal_sigma_m,
            vertical_sigma_m: self.vertical_sigma_m,
            velocity_sigma_mps: self.velocity_sigma_mps,
        })
    }
}

impl Gnss {
    pub fn from_toml_str(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }

    pub(crate) fn validate(&self, id: &str, path: &Path) -> Result<(), ConfigError> {
        let check = |sigmas: [(&'static str, f64); 3], scope: &str| -> Result<(), ConfigError> {
            for (name, value) in sigmas {
                require(value.is_finite() && value > 0.0, id, path, || {
                    format!("{scope}{name} must be finite and positive, got {value}")
                })?;
            }
            Ok(())
        };

        check(
            [
                ("horizontal_sigma_m", self.noise.horizontal_sigma_m),
                ("vertical_sigma_m", self.noise.vertical_sigma_m),
                ("velocity_sigma_mps", self.noise.velocity_sigma_mps),
            ],
            "noise.",
        )?;

        for (fix, noise) in &self.noise.by_fix {
            require(FIX_TYPES.contains(&fix.as_str()), id, path, || {
                format!(
                    "unknown fix type `{fix}`; expected one of {}",
                    FIX_TYPES.join(", ")
                )
            })?;
            check(
                [
                    ("horizontal_sigma_m", noise.horizontal_sigma_m),
                    ("vertical_sigma_m", noise.vertical_sigma_m),
                    ("velocity_sigma_mps", noise.velocity_sigma_mps),
                ],
                &format!("noise.by_fix.{fix}."),
            )?;
        }

        require(
            self.antenna_offset_m.iter().all(|v| v.is_finite()),
            id,
            path,
            || "antenna_offset_m must be finite".to_string(),
        )?;
        Ok(())
    }
}

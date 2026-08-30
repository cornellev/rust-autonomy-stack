//! Wheel encoders.

use std::path::Path;

use serde::Deserialize;

use super::require;
use crate::ConfigError;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WheelEncoder {
    /// Counts per mechanical revolution, before quadrature decoding.
    pub ticks_per_rev: u32,
    /// Rolling radius of the wheel this encoder is on, metres.
    pub wheel_radius_m: f64,
    /// Whether the decoder counts all four quadrature edges.
    #[serde(default)]
    pub quadrature: bool,
    pub noise: EncoderNoise,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EncoderNoise {
    /// Longitudinal slip as a fraction of distance travelled. This dominates the
    /// quantization term on any real surface.
    pub slip_sigma: f64,
    /// Extra tick-count noise beyond quantization, in ticks.
    #[serde(default)]
    pub tick_jitter: f64,
}

impl WheelEncoder {
    pub fn from_toml_str(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }

    /// Effective counts per revolution after quadrature decoding.
    pub fn effective_ticks_per_rev(&self) -> u32 {
        if self.quadrature {
            self.ticks_per_rev * 4
        } else {
            self.ticks_per_rev
        }
    }

    /// Distance represented by one tick, metres. Quantization is uniform over this
    /// interval, so its standard deviation is this divided by `sqrt(12)`.
    pub fn metres_per_tick(&self) -> f64 {
        2.0 * std::f64::consts::PI * self.wheel_radius_m / f64::from(self.effective_ticks_per_rev())
    }

    /// Standard deviation of the quantization error on a single tick, metres.
    pub fn quantization_sigma_m(&self) -> f64 {
        self.metres_per_tick() / 12f64.sqrt()
    }

    pub(crate) fn validate(&self, id: &str, path: &Path) -> Result<(), ConfigError> {
        require(self.ticks_per_rev > 0, id, path, || {
            "ticks_per_rev must be non-zero".to_string()
        })?;
        require(
            self.wheel_radius_m.is_finite() && self.wheel_radius_m > 0.0,
            id,
            path,
            || {
                format!(
                    "wheel_radius_m must be finite and positive, got {}",
                    self.wheel_radius_m
                )
            },
        )?;
        for (name, value) in [
            ("slip_sigma", self.noise.slip_sigma),
            ("tick_jitter", self.noise.tick_jitter),
        ] {
            require(value.is_finite() && value >= 0.0, id, path, || {
                format!("noise.{name} must be finite and non-negative, got {value}")
            })?;
        }
        Ok(())
    }
}

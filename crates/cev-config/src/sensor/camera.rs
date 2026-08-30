//! Cameras.

use std::path::Path;

use serde::Deserialize;

use super::require;
use crate::ConfigError;

/// A camera's physical parameters. Nothing here is meaningful for any other kind
/// of sensor, which is why it lives in its own struct behind [`Kind::Camera`].
///
/// [`Kind::Camera`]: super::Kind::Camera
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Camera {
    /// `[width, height]`, pixels.
    pub resolution: [u32; 2],
    pub model: Projection,
    pub intrinsics: Intrinsics,
    #[serde(default)]
    pub distortion: Distortion,
    #[serde(default)]
    pub shutter: Shutter,
    pub noise: CameraNoise,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Projection {
    Pinhole,
    Fisheye,
}

/// Pinhole intrinsics in pixels.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Intrinsics {
    pub fx: f64,
    pub fy: f64,
    pub cx: f64,
    pub cy: f64,
}

/// Lens distortion. The coefficient count is checked against the model, because a
/// `radtan` vector silently short by one element undistorts wrongly rather than
/// failing.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Distortion {
    #[serde(default)]
    pub model: DistortionModel,
    #[serde(default)]
    pub coeffs: Vec<f64>,
}

impl Default for Distortion {
    fn default() -> Self {
        Self {
            model: DistortionModel::None,
            coeffs: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DistortionModel {
    #[default]
    None,
    /// Brown-Conrady: `k1 k2 p1 p2 k3`.
    Radtan,
    /// Kannala-Brandt: `k1 k2 k3 k4`.
    Equidistant,
}

impl DistortionModel {
    /// How many coefficients this model requires.
    pub fn coeff_count(self) -> usize {
        match self {
            DistortionModel::None => 0,
            DistortionModel::Radtan => 5,
            DistortionModel::Equidistant => 4,
        }
    }
}

/// Shutter type. Rolling shutter needs its line readout time, without which
/// visual-inertial odometry cannot correct for motion during exposure.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Shutter {
    #[default]
    Global,
    Rolling {
        /// Seconds between successive line readouts.
        line_time_s: f64,
    },
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CameraNoise {
    /// Reprojection standard deviation in pixels. This is the number a
    /// visual-inertial filter squares into its measurement covariance `R`.
    pub pixel_sigma: f64,
    /// Standard deviation of the frame timestamp, seconds.
    #[serde(default)]
    pub timestamp_jitter_s: f64,
}

impl Camera {
    pub fn from_toml_str(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }

    /// Read a standalone camera parameter file.
    pub fn from_toml_file(path: &Path) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        toml::from_str(&text).map_err(|source| ConfigError::Parse {
            path: path.to_path_buf(),
            source,
        })
    }

    pub fn width(&self) -> u32 {
        self.resolution[0]
    }

    pub fn height(&self) -> u32 {
        self.resolution[1]
    }

    /// Row-major camera matrix `K`.
    pub fn k_matrix(&self) -> [[f64; 3]; 3] {
        let Intrinsics { fx, fy, cx, cy } = self.intrinsics;
        [[fx, 0.0, cx], [0.0, fy, cy], [0.0, 0.0, 1.0]]
    }

    pub(crate) fn validate(&self, id: &str, path: &Path) -> Result<(), ConfigError> {
        let [width, height] = self.resolution;
        require(width > 0 && height > 0, id, path, || {
            "resolution must be non-zero in both axes".to_string()
        })?;

        let Intrinsics { fx, fy, cx, cy } = self.intrinsics;
        require(fx.is_finite() && fx > 0.0, id, path, || {
            format!("intrinsics.fx must be finite and positive, got {fx}")
        })?;
        require(fy.is_finite() && fy > 0.0, id, path, || {
            format!("intrinsics.fy must be finite and positive, got {fy}")
        })?;
        require(
            cx.is_finite()
                && cy.is_finite()
                && (0.0..f64::from(width)).contains(&cx)
                && (0.0..f64::from(height)).contains(&cy),
            id,
            path,
            || format!("principal point ({cx}, {cy}) falls outside the {width}x{height} image"),
        )?;

        let expected = self.distortion.model.coeff_count();
        require(self.distortion.coeffs.len() == expected, id, path, || {
            format!(
                "distortion model `{:?}` needs {expected} coefficient(s), got {}",
                self.distortion.model,
                self.distortion.coeffs.len()
            )
        })?;
        require(
            self.distortion.coeffs.iter().all(|c| c.is_finite()),
            id,
            path,
            || "distortion coefficients must be finite".to_string(),
        )?;

        if let Shutter::Rolling { line_time_s } = self.shutter {
            require(
                line_time_s.is_finite() && line_time_s > 0.0,
                id,
                path,
                || format!("shutter.line_time_s must be finite and positive, got {line_time_s}"),
            )?;
        }

        require(
            self.noise.pixel_sigma.is_finite() && self.noise.pixel_sigma > 0.0,
            id,
            path,
            || {
                format!(
                    "noise.pixel_sigma must be finite and positive, got {}",
                    self.noise.pixel_sigma
                )
            },
        )?;
        require(
            self.noise.timestamp_jitter_s.is_finite() && self.noise.timestamp_jitter_s >= 0.0,
            id,
            path,
            || "noise.timestamp_jitter_s must be finite and non-negative".to_string(),
        )?;
        Ok(())
    }
}

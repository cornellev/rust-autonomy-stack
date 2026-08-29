//! Per-car deployment manifests.
//!
//! One `cars/<name>/car.toml` describes a single vehicle: which nodes it runs,
//! how they are launched, and where the binaries go when deployed. The manifest
//! is *deployment* truth only: vehicle geometry, CAN IDs and sensor extrinsics
//! are hardware truth and have no home in this schema yet.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("no car named `{name}` (looked in {dir})")]
    NoSuchCar { name: String, dir: PathBuf },
    #[error("reading {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("parsing {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("{path}: {message}")]
    Invalid { path: PathBuf, message: String },
}

/// A single vehicle's deployment manifest.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Car {
    /// Must match the directory name under `cars/`.
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Environment applied to every node, before per-node `env`.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Where `cev deploy` puts things. Absent means this car is launch-only.
    pub deploy: Option<Deploy>,
    #[serde(default, rename = "nodes")]
    pub nodes: Vec<Node>,
    /// Filled in by [`Car::load`]; not read from the file.
    #[serde(skip)]
    pub dir: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Deploy {
    /// Hostname or IP of the vehicle computer.
    pub host: String,
    #[serde(default = "default_user")]
    pub user: String,
    /// Rust target triple to build for, e.g. `aarch64-unknown-linux-gnu`.
    pub target: String,
    /// How to cross-compile. `cross` shells out to the `cross` tool; `cargo`
    /// uses the host toolchain with `--target` (needs a working linker).
    #[serde(default)]
    pub builder: Builder,
    /// Root of the install on the car. Binaries land in `<install_dir>/bin`,
    /// the car directory in `<install_dir>/config`.
    #[serde(default = "default_install_dir")]
    pub install_dir: String,
    /// Run over ssh after a successful sync, e.g. a systemctl restart.
    pub restart_cmd: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Builder {
    #[default]
    Cross,
    Cargo,
}

/// One process in the stack.
///
/// A node is either built from this workspace (`package`) or run as an external
/// command (`command`) -- exactly one, never both. `command` exists for nodes
/// that are not plain Cargo packages, such as the ROS 2 / colcon planner.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Node {
    /// Role name: estimation, control, planning, perception, logging.
    pub name: String,
    /// Cargo package to build and run.
    pub package: Option<String>,
    /// Binary within `package`; defaults to the package name.
    pub bin: Option<String>,
    /// External command, argv style. Mutually exclusive with `package`.
    #[serde(default)]
    pub command: Vec<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}
fn default_user() -> String {
    "cev".to_string()
}
fn default_install_dir() -> String {
    "/opt/cev".to_string()
}

impl Node {
    /// Binary name for a workspace node.
    pub fn bin_name(&self) -> Option<&str> {
        let package = self.package.as_deref()?;
        Some(self.bin.as_deref().unwrap_or(package))
    }

    /// Full argv for an external node.
    pub fn external_argv(&self) -> Option<Vec<String>> {
        if self.command.is_empty() {
            return None;
        }
        let mut argv = self.command.clone();
        argv.extend(self.args.iter().cloned());
        Some(argv)
    }
}

impl Car {
    /// Load `<cars_dir>/<name>/car.toml`.
    pub fn load(cars_dir: &Path, name: &str) -> Result<Self, ConfigError> {
        let dir = cars_dir.join(name);
        let path = dir.join("car.toml");
        if !path.exists() {
            return Err(ConfigError::NoSuchCar {
                name: name.to_string(),
                dir: cars_dir.to_path_buf(),
            });
        }
        let text = std::fs::read_to_string(&path).map_err(|source| ConfigError::Io {
            path: path.clone(),
            source,
        })?;
        let mut car: Car = toml::from_str(&text).map_err(|source| ConfigError::Parse {
            path: path.clone(),
            source,
        })?;
        car.dir = dir;
        car.validate(&path)?;
        Ok(car)
    }

    /// Every car directory under `cars_dir`, sorted.
    pub fn list(cars_dir: &Path) -> Result<Vec<String>, ConfigError> {
        let mut names = Vec::new();
        let entries = std::fs::read_dir(cars_dir).map_err(|source| ConfigError::Io {
            path: cars_dir.to_path_buf(),
            source,
        })?;
        for entry in entries {
            let entry = entry.map_err(|source| ConfigError::Io {
                path: cars_dir.to_path_buf(),
                source,
            })?;
            if entry.path().join("car.toml").is_file() {
                names.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        names.sort();
        Ok(names)
    }

    /// Nodes that are enabled, in manifest order.
    pub fn enabled_nodes(&self) -> impl Iterator<Item = &Node> {
        self.nodes.iter().filter(|node| node.enabled)
    }

    /// Cargo packages needed to launch this car.
    pub fn cargo_packages(&self) -> Vec<&str> {
        let mut packages: Vec<&str> = self
            .enabled_nodes()
            .filter_map(|node| node.package.as_deref())
            .collect();
        packages.sort_unstable();
        packages.dedup();
        packages
    }

    fn validate(&self, path: &Path) -> Result<(), ConfigError> {
        let invalid = |message: String| ConfigError::Invalid {
            path: path.to_path_buf(),
            message,
        };

        let dir_name = self
            .dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        if self.name != dir_name {
            return Err(invalid(format!(
                "name = \"{}\" does not match directory `{dir_name}`",
                self.name
            )));
        }

        let mut seen = BTreeMap::new();
        for node in &self.nodes {
            if seen.insert(node.name.clone(), ()).is_some() {
                return Err(invalid(format!("duplicate node `{}`", node.name)));
            }
            match (node.package.is_some(), !node.command.is_empty()) {
                (true, true) => {
                    return Err(invalid(format!(
                        "node `{}` sets both `package` and `command`; pick one",
                        node.name
                    )));
                }
                (false, false) => {
                    return Err(invalid(format!(
                        "node `{}` sets neither `package` nor `command`",
                        node.name
                    )));
                }
                _ => {}
            }
            if node.package.is_none() && node.bin.is_some() {
                return Err(invalid(format!(
                    "node `{}` sets `bin` without `package`",
                    node.name
                )));
            }
        }
        Ok(())
    }
}

//! Per-car deployment manifests.
//!
//! One `cars/<name>/car.toml` describes a single vehicle: which nodes it runs,
//! how they are launched, and where the binaries go when deployed. The manifest
//! is *deployment* truth only: vehicle geometry, CAN IDs and sensor extrinsics
//! are hardware truth and have no home in this schema yet.
//!
//! A node package may also ship `profiles.toml` beside its `Cargo.toml`. A car
//! names one of those profiles; the file holds the argv.

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
    /// Named arg list in the package's `profiles.toml`. Applied before `args`.
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Skip check, launch, and deploy when `package` is not on disk.
    #[serde(default)]
    pub optional: bool,
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

const PROFILES_FILE: &str = "profiles.toml";

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct LaunchProfile {
    #[serde(default)]
    args: Vec<String>,
}

/// Args for `name` in `<package_dir>/profiles.toml`.
///
/// Cargo does not read this file. Each top-level table is one profile.
pub fn profile_args(package_dir: &Path, name: &str) -> Result<Vec<String>, ConfigError> {
    let path = package_dir.join(PROFILES_FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Err(ConfigError::Invalid {
                path: package_dir.to_path_buf(),
                message: format!("profile `{name}` is not defined (no {})", path.display()),
            });
        }
        Err(source) => {
            return Err(ConfigError::Io {
                path: path.clone(),
                source,
            });
        }
    };
    args_in(&text, name, &path)
}

fn args_in(text: &str, name: &str, path: &Path) -> Result<Vec<String>, ConfigError> {
    let profiles: BTreeMap<String, LaunchProfile> =
        toml::from_str(text).map_err(|source| ConfigError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
    match profiles.get(name) {
        Some(profile) => Ok(profile.args.clone()),
        None => {
            let defined = if profiles.is_empty() {
                "none".to_string()
            } else {
                profiles.keys().cloned().collect::<Vec<_>>().join(", ")
            };
            Err(ConfigError::Invalid {
                path: path.to_path_buf(),
                message: format!("no profile `{name}` (defined: {defined})"),
            })
        }
    }
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

    /// Enabled nodes, omitting optional packages that are not in `present`.
    pub fn runnable<'a>(&'a self, present: &[&str]) -> Vec<&'a Node> {
        self.enabled_nodes()
            .filter(|node| match node.package.as_deref() {
                Some(package) if node.optional && !present.contains(&package) => false,
                _ => true,
            })
            .collect()
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
            if let Some(profile) = &node.profile {
                if node.package.is_none() {
                    return Err(invalid(format!(
                        "node `{}` sets `profile` without `package`",
                        node.name
                    )));
                }
                if profile.is_empty() {
                    return Err(invalid(format!(
                        "node `{}` sets an empty `profile`",
                        node.name
                    )));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dry_run_profile_args() {
        let dir = std::env::temp_dir().join(format!("cev-profiles-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("profiles.toml"),
            "[dry-run]\nargs = [\"--dry-run\"]\n",
        )
        .unwrap();

        assert_eq!(
            profile_args(&dir, "dry-run").unwrap(),
            vec!["--dry-run".to_string()]
        );
        let missing = profile_args(&dir, "live").unwrap_err().to_string();
        assert!(missing.contains("live"), "{missing}");
        assert!(missing.contains("dry-run"), "{missing}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn profile_requires_package() {
        let root = std::env::temp_dir().join(format!("cev-car-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("sim");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("car.toml"),
            r#"
name = "sim"

[[nodes]]
name = "planner"
command = ["ros2", "run", "pkg", "node"]
profile = "dry-run"
"#,
        )
        .unwrap();

        let err = Car::load(&root, "sim").unwrap_err().to_string();
        assert!(err.contains("profile"), "{err}");
        assert!(err.contains("package"), "{err}");

        let _ = std::fs::remove_dir_all(&root);
    }

    fn node(name: &str, package: &str, optional: bool) -> Node {
        Node {
            name: name.to_string(),
            package: Some(package.to_string()),
            bin: None,
            command: Vec::new(),
            profile: None,
            args: Vec::new(),
            env: BTreeMap::new(),
            enabled: true,
            optional,
        }
    }

    #[test]
    fn optional_package_runs_only_when_present() {
        let car = Car {
            name: "sim".to_string(),
            description: String::new(),
            env: BTreeMap::new(),
            deploy: None,
            nodes: vec![
                node("control", "pure_pursuit", false),
                node("deadreckoning", "cev_odometry", true),
            ],
            dir: PathBuf::new(),
        };

        let absent: Vec<_> = car
            .runnable(&["pure_pursuit"])
            .iter()
            .map(|node| node.name.as_str())
            .collect();
        assert_eq!(absent, ["control"]);

        let present: Vec<_> = car
            .runnable(&["pure_pursuit", "cev_odometry"])
            .iter()
            .map(|node| node.name.as_str())
            .collect();
        assert_eq!(present, ["control", "deadreckoning"]);
    }
}

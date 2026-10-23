//! Node contracts.
//!
//! Every node repository ships a `cev-node.toml` at the root of its package:
//! what it consumes, what it produces, how it runs, on what hardware, and in
//! which modes. The orchestrator never routes data; it reads these to check
//! that a car's nodes fit together before anything is launched.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::ConfigError;

/// File name every node repository needs to provide at the package root.
pub const CONTRACT_FILE: &str = "cev-node.toml";

/// What a node repository promises the rest of the stack.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeContract {
    /// Bumped when this schema changes incompatibly.
    pub contract_version: u32,
    ///Role this node fills: estimation, control, planning, perception, etc.
    pub role: String,
    #[serde(default)]
    pub description: String,
    pub run: Run,
    pub compute: Compute,
    /// Modes this node runs in. Required and explicit: a node is off in
    /// any mode it does not name, so a new mode never starts a controller by accident.
    pub modes: Vec<Mode>,
    #[serde(default)]
    pub inputs: Vec<Input>,
    #[serde(default)]
    pub outputs: Vec<Output>,
    #[serde(default)]
    pub metrics: Vec<Metric>,
    /// Filled in by [`NodeContract::load`]; not read from the file.
    #[serde(skip)]
    pub path: PathBuf,
}

/// How to start the node: a workspace `package`, or an external `command`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Run {
    pub package: Option<String>,
    /// Binary within `package`; defaults to the package name.
    pub bin: Option<String>,
    /// External command, argv style. Mutually exclusive with `package`.
    #[serde(default)]
    pub command: Vec<String>,
    #[serde(default)]
    pub args: Vec<String>,
}

/// Where the node computes. A GPU node must say which model it loads and
/// which runtime executes it; the type makes a bare `device = "gpu"` a parse
/// error rather than something to check later.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "device", rename_all = "lowercase")]
pub enum Compute {
    Cpu,
    Gpu { model: String, runtime: String }, //NOTE: this should have an optional model, as this should be a check when building the package that it exists and not the other way around.
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Teleop,
    Autonomous,
    Efficiency,
}

/// Whether the stack is driving a simulator or real vehicle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunKind {
    Sim,
    Real,
}

/// A topic this node subscribes to.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    pub topic: String,
    #[serde(rename = "type")]
    pub msg_type: String,
    /// Slowest producer rate this node can tolerate.
    pub min_rate_hz: f64,
}

/// A topic this node publishes.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Output {
    pub topic: String,
    #[serde(rename = "type")]
    pub msg_type: String,
    pub rate_hz: f64,
}

/// A metric that needs to be published.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Metric {
    pub name: String,
    pub when: Vec<RunKind>,
}

impl NodeContract {
    /// Parse a contract from text. `path` is only used in error messages.
    pub fn from_toml_str(text: &str, path: &Path) -> Result<Self, ConfigError> {
        let mut contract: NodeContract =
            toml::from_str(text).map_err(|source| ConfigError::Parse {
                path: path.to_path_buf(),
                source,
            })?;
        contract.path = path.to_path_buf();
        Ok(contract)
    }

    /// Read and parse the contract at \path`.`
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Self::from_toml_str(&text, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ESTIMATION: &str = r#"
        contract_version = 1
        role = "estimation"
        modes = ["teleop", "autonomous", "efficiency"]

        [run]
        package = "rust-inekf"

        [compute]
        device = "cpu"

        [[inputs]]
        topic = "/sensors/imu"
        type = "cev_msgs/IMU"
        min_rate_hz = 200.0

        [[outputs]]
        topic = "/estimation/state"
        type = "cev_msgs/Odometry"
        rate_hz = 200.0

        [[metrics]]
        name = "nis"
        when = ["sim", "real"]
    "#;

    fn parse(text: &str) -> Result<NodeContract, ConfigError> {
        NodeContract::from_toml_str(text, Path::new("test.toml"))
    }

    #[track_caller]
    fn parse_ok(text: &str) -> NodeContract {
        match parse(text) {
            Ok(contract) => contract,
            Err(err) => panic!("{err}"),
        }
    }

    #[test]
    fn parses_estimation() {
        let contract = parse_ok(ESTIMATION);
        assert_eq!(contract.role, "estimation");
        assert_eq!(contract.compute, Compute::Cpu);
        assert_eq!(contract.modes.len(), 3);
        assert_eq!(contract.inputs[0].msg_type, "cev_msgs/IMU");
        assert_eq!(contract.outputs[0].rate_hz, 200.0);
        assert_eq!(contract.metrics[0].when, vec![RunKind::Sim, RunKind::Real]);
    }

    #[test]
    fn parses_gpu_node() {
        let text = r#"
            contract_version = 1
            role = "perception"
            modes = ["autonomous"]

            [run]
            command = ["python3", "-m", "detector"]

            [compute]
            device = "gpu"
            model = "models/yolo.onnx"
            runtime = "tensorrt"
        "#;
        let contract = parse_ok(text);
        assert_eq!(
            contract.compute,
            Compute::Gpu {
                model: "models/yolo.onnx".to_string(),
                runtime: "tensorrt".to_string(),
            }
        );
        assert!(contract.inputs.is_empty());
    }

    #[test]
    fn gpu_without_model_is_rejected() {
        let text = ESTIMATION.replace(r#"device = "cpu""#, r#"device = "gpu""#);
        assert!(parse(&text).is_err());
    }

    #[test]
    fn unknown_mode_is_rejected() {
        let text = ESTIMATION.replace("\"teleop\"", "\"teleopp\"");
        assert!(parse(&text).is_err());
    }

    #[test]
    fn missing_modes_is_rejected() {
        let text = ESTIMATION.replace(r#"modes = ["teleop", "autonomous", "efficiency"]"#, "");
        assert!(parse(&text).is_err());
    }
}

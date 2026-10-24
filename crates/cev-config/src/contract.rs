//! Node contracts.
//!
//! Every node repository ships a `cev-node.toml` at the root of its package:
//! what it consumes, what it produces, how it runs, on what hardware, and in
//! which modes. The orchestrator never routes data; it reads these to check
//! that a car's nodes fit together before anything is launched.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use sensor_defs::MsgType;

use crate::ConfigError;

/// File name every node repository needs to provide at the package root.
pub const CONTRACT_FILE: &str = "cev-node.toml";
pub const CONTRACT_VERSION: u32 = 1;

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

/// Where the node computes. A GPU node must name the runtime it needs
/// (`cuda`, `tensorrt`, `onnx`); `model` is only for nodes that load one,
/// since some GPU nodes are plain parallel kernels with no model.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "device", rename_all = "lowercase")]
pub enum Compute {
    Cpu,
    Gpu {
        model: Option<String>,
        runtime: String,
    },
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

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ContractProblem {
    #[error("contract_version = {found}, but this cev reads version {expected}")]
    UnsupportedVersion { found: u32, expected: u32 },
    #[error("`run` sets both `package` and `command`; pick one")]
    BothPackageAndCommand,
    #[error("`run` sets neither `package` nor `command`")]
    NeitherPackageNorCommand,
    #[error("`run.bin` is set without `run.package`")]
    BinWithoutPackage,
    #[error("`modes` is empty, so this node never runs")]
    NoModes,
    #[error("topic `{topic}`: {rate_hz} Hz is not a positive, finite rate")]
    BadRate { topic: String, rate_hz: f64 },
    #[error("topic `{topic}` is not a usable Zenoh key: {reason}")]
    BadTopic { topic: String, reason: String },
    #[error("topic `{topic}`: unknown type `{msg_type}` (not a sensor_defs::MsgType)")]
    UnknownType { topic: String, msg_type: String },
    #[error("topic `{topic}` is listed twice in `{section}`")]
    DuplicateTopic { section: String, topic: String },
}

impl NodeContract {
    /// Parse a contract from text. `path` is only used in error messages.
    pub fn from_toml_str(text: &str, path: &Path) -> Result<Self, ConfigError> {
        let mut contract: NodeContract =
            toml::from_str(text).map_err(|source| ConfigError::Parse {
                path: path.to_path_buf(),
                source,
            })?;
        let problems = contract.problems();
        if !problems.is_empty() {
            let mut message = String::from("contract violations: ");
            for problem in &problems {
                message.push_str("\n - ");
                message.push_str(&problem.to_string());
            }
            return Err(ConfigError::Invalid {
                path: path.to_path_buf(),
                message,
            });
        }
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

    /// Everything wrong with this contract, checked in isolation: no other
    /// node is consulted. Emtpy means valid.
    /// Each case is checked independently and pushed into a vector of problem enums.
    pub fn problems(&self) -> Vec<ContractProblem> {
        let mut problems = Vec::new();

        if self.contract_version != CONTRACT_VERSION {
            problems.push(ContractProblem::UnsupportedVersion {
                found: self.contract_version,
                expected: CONTRACT_VERSION,
            });
        }

        let run = &self.run;
        match (run.package.is_some(), !run.command.is_empty()) {
            (true, true) => problems.push(ContractProblem::BothPackageAndCommand),
            (false, false) => problems.push(ContractProblem::NeitherPackageNorCommand),
            _ => {}
        }
        if run.package.is_none() && run.bin.is_some() {
            problems.push(ContractProblem::BinWithoutPackage);
        }

        if self.modes.is_empty() {
            problems.push(ContractProblem::NoModes);
        }

        for input in &self.inputs {
            if let Some(problem) = topic_problem(&input.topic) {
                problems.push(problem);
            }
        }

        for output in &self.outputs {
            if let Some(problem) = topic_problem(&output.topic) {
                problems.push(problem);
            }
        }

        for input in &self.inputs {
            if let Some(problem) = type_problem(&input.topic, &input.msg_type) {
                problems.push(problem);
            }
        }

        for output in &self.outputs {
            if let Some(problem) = type_problem(&output.topic, &output.msg_type) {
                problems.push(problem);
            }
        }

        for input in &self.inputs {
            if let Some(problem) = rate_problem(&input.topic, input.min_rate_hz) {
                problems.push(problem);
            }
        }

        for output in &self.outputs {
            if let Some(problem) = rate_problem(&output.topic, output.rate_hz) {
                problems.push(problem);
            }
        }

        let mut seen = HashSet::new();
        for input in &self.inputs {
            if !seen.insert(input.topic.as_str()) {
                problems.push(ContractProblem::DuplicateTopic {
                    section: "inputs".to_string(),
                    topic: input.topic.clone(),
                });
            }
        }

        let mut seen = HashSet::new();
        for output in &self.outputs {
            if !seen.insert(output.topic.as_str()) {
                problems.push(ContractProblem::DuplicateTopic {
                    section: "outputs".to_string(),
                    topic: output.topic.clone(),
                })
            }
        }

        problems
    }
}

/// A topic must be a valid Zenooh key expression with no wildcards. Zenoh
/// itself forbids empty chunks (so no leading, trailing or doubled `/`)
/// and the characters `#` and `?`. We also forbid `*` and `$`: a wildcard key
/// matches many topics, and the graph check needs each topic to name exactly one.
fn topic_problem(topic: &str) -> Option<ContractProblem> {
    let reason = if topic.starts_with('/') || topic.ends_with('/') {
        "no leading or trailing `/`"
    } else if topic.split('/').any(|chunk| chunk.is_empty()) {
        "no empty chunks (`//`, or an empty topic)"
    } else if topic.contains(['#', '?']) {
        "`#` and `?` are forbidden"
    } else if topic.contains(['*', '$']) {
        "wildcards (`*`, `**`, `$*`) are not allowed in a contract"
    } else {
        return None;
    };
    Some(ContractProblem::BadTopic {
        topic: topic.to_string(),
        reason: reason.to_string(),
    })
}

///  A message type msut be one of `sensor_defs` known types, so btoh ends of a topic
///  name a type that actually exists, not just the same string.
fn type_problem(topic: &str, msg_type: &str) -> Option<ContractProblem> {
    if MsgType::from_name(msg_type).is_some() {
        None
    } else {
        Some(ContractProblem::UnknownType {
            topic: topic.to_string(),
            msg_type: msg_type.to_string(),
        })
    }
}

/// Check that rate_hz is finite and not NAN
fn rate_problem(topic: &str, rate_hz: f64) -> Option<ContractProblem> {
    match rate_hz {
        _ if rate_hz.is_finite() && rate_hz > 0.0 => None,
        _ => Some(ContractProblem::BadRate {
            topic: topic.to_string(),
            rate_hz,
        }),
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
        topic = "sensors/imu"
        type = "IMU"
        min_rate_hz = 200.0

        [[outputs]]
        topic = "estimation/state"
        type = "StateEstimate"
        rate_hz = 200.0

        [[metrics]]
        name = "nis"
        when = ["sim", "real"]
    "#;

    /// Deserialize without running [`NodeContract::problems`] so tests can
    /// hold an invalid contract and inspect what's wrong with it.
    fn unchecked(text: &str) -> NodeContract {
        toml::from_str::<NodeContract>(text).unwrap_or_else(|err| panic!("{err}"))
    }

    fn parse(text: &str) -> Result<NodeContract, ConfigError> {
        NodeContract::from_toml_str(text, Path::new("test.toml"))
    }

    /// `str::replace`, but panics if `from` is not in `text`.     
    #[track_caller]
    fn edit(text: &str, from: &str, to: &str) -> String {
        assert!(text.contains(from), "fixture does not contain {from:?}");
        text.replace(from, to)
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
        assert_eq!(contract.inputs[0].msg_type, "IMU");
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
                model: Some("models/yolo.onnx".to_string()),
                runtime: "tensorrt".to_string(),
            }
        );
        assert!(contract.inputs.is_empty());
    }

    #[test]
    fn gpu_without_model_is_accepted() {
        let text = edit(
            ESTIMATION,
            r#"device = "cpu""#,
            r#"device = "gpu"
            runtime = "cuda""#,
        );
        let contract = parse_ok(&text);
        assert_eq!(
            contract.compute,
            Compute::Gpu {
                model: None,
                runtime: "cuda".to_string()
            }
        );
    }

    #[test]
    fn gpu_without_runtime_is_rejected() {
        let text = edit(ESTIMATION, r#"device = "cpu""#, r#"device = "gpu""#);
        assert!(parse(&text).is_err());
    }

    #[test]
    fn unknown_mode_is_rejected() {
        let text = edit(ESTIMATION, "\"teleop\"", "\"teleopp\"");
        assert!(parse(&text).is_err());
    }

    #[test]
    fn missing_modes_is_rejected() {
        let text = edit(
            ESTIMATION,
            r#"modes = ["teleop", "autonomous", "efficiency"]"#,
            "",
        );
        assert!(parse(&text).is_err());
    }

    #[test]
    fn valid_contract_has_no_problems() {
        assert_eq!(unchecked(ESTIMATION).problems(), vec![]);
    }

    #[test]
    fn wrong_version_is_problem() {
        let text = edit(ESTIMATION, "contract_version = 1", "contract_version = 2");
        assert_eq!(
            unchecked(&text).problems(),
            vec![ContractProblem::UnsupportedVersion {
                found: 2,
                expected: CONTRACT_VERSION,
            }]
        );
    }

    #[test]
    fn package_and_command_are_exclusive() {
        let both = edit(
            ESTIMATION,
            r#"package = "rust-inekf""#,
            r#"package = "rust-inekf" 
            command = ["inekf-node"]"#
        );
        assert_eq!(
            unchecked(&both).problems(),
            vec![ContractProblem::BothPackageAndCommand]
        );

        let neither = edit(ESTIMATION, r#"package = "rust-inekf""#, "");

        assert_eq!(
            unchecked(&neither).problems(),
            vec![ContractProblem::NeitherPackageNorCommand]
        );
    }

    #[test]
    fn bin_needs_package() {
        let text = edit(
            ESTIMATION,
            r#"package = "rust-inekf""#,
            r#"command = ["inekf"]
            bin = "inekf""#,
        );
        assert_eq!(
            unchecked(&text).problems(),
            vec![ContractProblem::BinWithoutPackage]
        );
    }

    #[test]
    fn empty_modes_is_a_problem() {
        let text = edit(
            ESTIMATION,
            r#"modes = ["teleop", "autonomous", "efficiency"]"#,
            "modes = []",
        );
        assert_eq!(unchecked(&text).problems(), vec![ContractProblem::NoModes]);
    }

    #[test]
    fn bad_rates_are_problems() {
        for bad in ["0.0", "-5.0", "nan", "inf"] {
            let text = edit(ESTIMATION, "rate_hz = 200.0", &format!("rate_hz = {bad}"));
            let problems = unchecked(&text).problems();
            // Both `min_rate_hz` and `rate_hz` end in `rate_hz = 200.0`
            assert_eq!(problems.len(), 2, "rate: {bad}: {problems:?}");
            for problem in &problems {
                assert!(
                    matches!(problem, ContractProblem::BadRate { .. }),
                    "rate {bad}: {problem}"
                );
            }
        }
    }

    #[test]
    fn duplicate_topic_is_a_problem() {
        let text = format!(
            "{ESTIMATION}
            [[inputs]]
            topic = \"sensors/imu\"
            type = \"IMU\"
            min_rate_hz = 50.0
            "
        );

        assert_eq!(
            unchecked(&text).problems(),
            vec![ContractProblem::DuplicateTopic {
                section: "inputs".to_string(),
                topic: "sensors/imu".to_string(),
            }]
        );
    }

    #[test]
    fn all_problems_reported_together() {
        let text = edit(ESTIMATION, r#"package = "rust-inekf""#, "");
        let text = edit(
            &text,
            r#"modes = ["teleop", "autonomous", "efficiency"]"#,
            "modes = []",
        );
        assert_eq!(unchecked(&text).problems().len(), 2);
    }

    #[test]
    fn parsing_rejects_invalid_contract() {
        let text = edit(
            ESTIMATION,
            r#"modes = ["teleop", "autonomous", "efficiency"]"#,
            "modes = []",
        );
        let err = parse(&text).unwrap_err().to_string();
        assert!(err.contains("`modes` is empty"), "{err}");
    }

    #[test]
    fn bad_topics_are_problems() {
        for bad in [
            "",
            "/sensors/imu",
            "sensors/imu/",
            "sensors//imu",
            "sensors/*",
            "sensors/**",
            "sensors/$*imu",
            "sensors/imu?",
            "sensors#imu",
        ] {
            assert!(
                matches!(topic_problem(bad), Some(ContractProblem::BadTopic { .. }),),
                "{bad:?} should be rejected"
            );
        }
    }

    #[test]
    fn zenoh_keys_are_accepted() {
        for good in ["sensors/imu", "estimation/state", "a", "lidar/front-left_0"] {
            assert_eq!(topic_problem(good), None, "{good}");
        }
    }

    #[test]
    fn bad_topic_in_contract_is_problem() {
        let text = edit(
            ESTIMATION,
            r#"topic = "sensors/imu""#,
            r#"topic = "/sensors/imu""#,
        );
        let problems = unchecked(&text).problems();
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(matches!(problems[0], ContractProblem::BadTopic { .. }));
    }

    #[test]
    fn unknown_type_is_a_problem() {
        let text = edit(ESTIMATION, r#"type = "IMU""#, r#"type = "cev_msgs/IMU""#);
        assert_eq!(
            unchecked(&text).problems(),
            vec![ContractProblem::UnknownType {
                topic: "sensors/imu".to_string(),
                msg_type: "cev_msgs/IMU".to_string()
            }]
        );
    }
}

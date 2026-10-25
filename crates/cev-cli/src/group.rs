//! Locating the stack root and the Cargo packages that it resolves.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

/// Walk up from `start` by looking for the stack root: the directory holding both
/// a `Cargo.toml` file and a `cars/` directory.
pub fn find_root(start: &Path) -> Result<PathBuf> {
    for dir in start.ancestors() {
        if dir.join("Cargo.toml").is_file() && dir.join("cars").exists() {
            return Ok(dir.to_path_buf());
        }
    }
    bail!(
        "not inside the stack (no directory above {} holds both Cargo.toml and cars/)",
        start.display()
    )
}

pub fn cwd_root() -> Result<PathBuf> {
    let cwd = std::env::current_dir().context("reading current directory")?;
    find_root(&cwd)
}

/// A cargo package in the resolved Dependency graph.
pub struct Package {
    pub name: String,
    pub dir: PathBuf,
}

#[derive(Deserialize)]
struct MetadataPackage {
    name: String,
    manifest_path: PathBuf,
}

/// The part of `cargo metadata --format-version 1` we read. No
/// `deny_unknown_fields`: cargo adds fields over time.
#[derive(Deserialize)]
struct Metadata {
    packages: Vec<MetadataPackage>,
}

/// Every package Cargo resolves for the workspace, including the node repositories.
/// Pulled in as git dependencies of `crates/cev-nodes`.
pub fn packages(root: &Path) -> Result<Vec<Package>> {
    let output = Command::new("cargo")
        .current_dir(root)
        .args(["metadata", "--format-version", "1"])
        .output()
        .context("running `cargo metadata`")?;
    if !output.status.success() {
        bail!(
            "`cargo metadata` failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let metadata: Metadata =
        serde_json::from_slice(&output.stdout).context("parsing `cargo metadata` output")?;

    let mut found = Vec::new();
    for package in metadata.packages {
        let Some(dir) = package.manifest_path.parent() else {
            continue;
        };
        found.push(Package {
            name: package.name,
            dir: dir.to_path_buf(),
        });
    }
    found.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(found)
}

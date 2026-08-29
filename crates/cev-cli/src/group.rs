//! Locating the repository group and the packages inside it.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// Walk up from `start` looking for the group root: the directory holding both
/// `gitman.yml` and `cars/`. Works from anywhere inside the group, including
/// from inside a cloned node repo.
pub fn find_root(start: &Path) -> Result<PathBuf> {
    for dir in start.ancestors() {
        if dir.join("gitman.yml").is_file() && dir.join("cars").is_dir() {
            return Ok(dir.to_path_buf());
        }
    }
    bail!(
        "not inside a repository group (no directory above {} holds both gitman.yml and cars/)",
        start.display()
    )
}

pub fn cwd_root() -> Result<PathBuf> {
    let cwd = std::env::current_dir().context("reading current directory")?;
    find_root(&cwd)
}

/// A Cargo package visible to the workspace build.
pub struct Package {
    pub name: String,
    pub dir: PathBuf,
}

/// Scan `crates/*` and `nodes/*` for Cargo packages.
///
/// This reads manifests directly rather than shelling out to `cargo metadata`,
/// so it still reports something useful when a `nodes/` symlink is dangling --
/// which is exactly the state `cev check` exists to catch.
pub fn packages(root: &Path) -> Result<Vec<Package>> {
    let mut found = Vec::new();
    for parent in ["crates", "nodes"] {
        let parent_dir = root.join(parent);
        let Ok(entries) = std::fs::read_dir(&parent_dir) else {
            continue; // nodes/ does not exist until `gitman install` has run
        };
        for entry in entries {
            let dir = entry?.path();
            let manifest = dir.join("Cargo.toml");
            if !manifest.is_file() {
                continue;
            }
            if let Some(name) = package_name(&manifest)? {
                found.push(Package { name, dir });
            }
        }
    }
    found.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(found)
}

/// The `name` from a `[package]` table, if the manifest has one.
fn package_name(manifest: &Path) -> Result<Option<String>> {
    let text = std::fs::read_to_string(manifest)
        .with_context(|| format!("reading {}", manifest.display()))?;
    let mut in_package = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
            continue;
        }
        if !in_package {
            continue;
        }
        if let Some(value) = line.strip_prefix("name") {
            let value = value.trim_start().trim_start_matches('=').trim();
            return Ok(Some(value.trim_matches('"').to_string()));
        }
    }
    Ok(None)
}

/// Dangling symlinks under `nodes/`, which break the workspace glob.
pub fn broken_links(root: &Path) -> Result<Vec<PathBuf>> {
    let nodes = root.join("nodes");
    let Ok(entries) = std::fs::read_dir(&nodes) else {
        return Ok(Vec::new());
    };
    let mut broken = Vec::new();
    for entry in entries {
        let path = entry?.path();
        if !path.exists() {
            broken.push(path);
        }
    }
    Ok(broken)
}

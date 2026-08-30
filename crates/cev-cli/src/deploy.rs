//! Pushing a built stack onto a car.
//!
//! Deliberately thin: cross-compile, rsync, restart. Everything it does is
//! printed first, and `--dry-run` prints without running.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use cev_config::{Builder, Car};

use crate::launch::{build_packages, render};

pub fn deploy(root: &Path, car: &Car, build: bool, dry_run: bool) -> Result<()> {
    let Some(deploy) = &car.deploy else {
        bail!(
            "car `{}` has no [deploy] section; it is launch-only",
            car.name
        );
    };
    let remote = format!("{}@{}", deploy.user, deploy.host);
    let packages = car.cargo_packages();

    let external: Vec<&str> = car
        .enabled_nodes()
        .filter(|node| node.package.is_none())
        .map(|node| node.name.as_str())
        .collect();
    if !external.is_empty() {
        println!(
            "[cev] note: {} built by its own toolchain, not deployed by cev",
            external.join(", ")
        );
    }

    if build && !packages.is_empty() {
        let builder = match deploy.builder {
            Builder::Cross => "cross",
            Builder::Cargo => "cargo",
        };
        if dry_run {
            println!(
                "[cev] would build {} for {}",
                packages.join(", "),
                deploy.target
            );
        } else {
            build_packages(root, &packages, Some((builder, &deploy.target)))?;
        }
    }

    let mut binaries = Vec::new();
    for node in car.enabled_nodes() {
        let Some(bin) = node.bin_name() else { continue };
        let path = root
            .join("target")
            .join(&deploy.target)
            .join("release")
            .join(bin);
        if !dry_run && !path.is_file() {
            bail!("{} not built for {}", path.display(), deploy.target);
        }
        binaries.push(path);
    }

    let bin_dir = format!("{}/bin", deploy.install_dir);
    let config_dir = format!("{}/config", deploy.install_dir);
    let sensors_dir = format!("{}/sensors", deploy.install_dir);

    let mut mkdir = Command::new("ssh");
    mkdir
        .arg(&remote)
        .arg(format!("mkdir -p {bin_dir} {config_dir} {sensors_dir}"));
    run(mkdir, dry_run)?;

    if !binaries.is_empty() {
        let mut rsync = Command::new("rsync");
        rsync.arg("-az").arg("--info=progress2");
        rsync.args(&binaries);
        rsync.arg(format!("{remote}:{bin_dir}/"));
        run(rsync, dry_run)?;
    }

    // Trailing slash on the source: copy the contents of cars/<name>/, not the
    // directory itself. --delete so a removed config does not linger on the car.
    let mut rsync = Command::new("rsync");
    rsync
        .arg("-az")
        .arg("--delete")
        .arg(format!("{}/", car.dir.display()))
        .arg(format!("{remote}:{config_dir}/"));
    run(rsync, dry_run)?;

    // The sensor catalog is group-level, so it cannot ride along inside
    // cars/<name>/ -- and it cannot be merged into config/ either, because the
    // rsync above runs with --delete and would remove it again. Its own
    // destination, matching CEV_SENSORS_DIR.
    let catalog = root.join("sensors");
    if catalog.is_dir() {
        let mut rsync = Command::new("rsync");
        rsync
            .arg("-az")
            .arg("--delete")
            .arg(format!("{}/", catalog.display()))
            .arg(format!("{remote}:{sensors_dir}/"));
        run(rsync, dry_run)?;
    }

    if let Some(restart) = &deploy.restart_cmd {
        let mut ssh = Command::new("ssh");
        ssh.arg(&remote).arg(restart);
        run(ssh, dry_run)?;
    }

    if !dry_run {
        println!(
            "[cev] deployed {} to {remote}:{}",
            car.name, deploy.install_dir
        );
    }
    Ok(())
}

fn run(mut command: Command, dry_run: bool) -> Result<()> {
    let rendered = render(&command);
    if dry_run {
        println!("[cev] would run: {rendered}");
        return Ok(());
    }
    println!("[cev] {rendered}");
    let status = command
        .status()
        .with_context(|| format!("running `{rendered}`"))?;
    if !status.success() {
        bail!("`{rendered}` failed: {status}");
    }
    Ok(())
}

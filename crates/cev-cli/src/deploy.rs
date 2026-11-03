//! Pushing a built stack onto a car.
//!
//! Deliberately thin: cross-compile, rsync the binaries and a launch script,
//! restart. `--pre-build` stops after the compile. Everything it does is printed
//! first, and `--dry-run` prints without running.

use std::path::Path;
use std::process::{Command, ExitStatus};

use anyhow::{Context, Result, bail};
use cev_config::{Builder, Car};

use crate::launch::{
    LaunchNode, build_car, launch_nodes, render, render_kill_script, render_launch_script,
    script_car_stamp, shell_single_quote,
};

pub fn deploy(root: &Path, car: &Car, build: bool, dry_run: bool, pre_build: bool) -> Result<()> {
    let Some(deploy) = &car.deploy else {
        bail!(
            "car `{}` has no [deploy] section; it is launch-only",
            car.name
        );
    };
    let remote = format!("{}@{}", deploy.user, deploy.host);
    let workspace = crate::group::packages(root)?;
    let present: Vec<&str> = workspace.iter().map(|pkg| pkg.name.as_str()).collect();
    let nodes = car.runnable(&present);

    let external: Vec<&str> = nodes
        .iter()
        .filter(|node| node.package.is_none())
        .map(|node| node.name.as_str())
        .collect();
    if !external.is_empty() {
        println!(
            "[cev] note: {} built by its own toolchain, not deployed by cev",
            external.join(", ")
        );
    }

    if build {
        let builder = match deploy.builder {
            Builder::Cross => "cross",
            Builder::Cargo => "cargo",
        };
        build_car(
            root,
            &nodes,
            &workspace,
            Some((builder, &deploy.target)),
            dry_run,
        )?;
    }

    let mut binaries = Vec::new();
    for node in &nodes {
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

    if pre_build {
        if !dry_run {
            println!("[cev] pre-built {} for {}", car.name, deploy.target);
        }
        return Ok(());
    }

    // Resolve profiles before any ssh. A missing profile fails the deploy
    // instead of shipping binaries the car cannot start.
    let launching = launch_nodes(car, &workspace)?;

    let bin_dir = format!("{}/bin", deploy.install_dir);
    let config_dir = format!("{}/config", deploy.install_dir);

    let mut mkdir = Command::new("ssh");
    mkdir
        .arg(&remote)
        .arg(format!("mkdir -p {bin_dir} {config_dir}"));
    run(mkdir, dry_run)?;

    if !binaries.is_empty() {
        let mut rsync = Command::new("rsync");
        rsync.arg("-az").arg("--info=progress2");
        rsync.args(&binaries);
        rsync.arg(format!("{remote}:{bin_dir}/"));
        run(rsync, dry_run)?;
    }

    install_launch_script(car, &launching, &remote, &deploy.install_dir, dry_run)?;

    // Trailing slash on the source: copy the contents of cars/<name>/, not the
    // directory itself. --delete so a removed config does not linger on the car.
    let mut rsync = Command::new("rsync");
    rsync
        .arg("-az")
        .arg("--delete")
        .arg(format!("{}/", car.dir.display()))
        .arg(format!("{remote}:{config_dir}/"));
    run(rsync, dry_run)?;

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

pub fn remote_launch(car: &Car) -> Result<()> {
    let installed = installed(car)?;
    let mut ssh = Command::new("ssh");
    ssh.arg("-t")
        .arg(&installed.remote)
        .arg(format!("exec {}", shell_single_quote(&installed.script)));
    // 129/130/143: the script caught HUP/INT/TERM and stopped the stack.
    let status = ssh_in_foreground(ssh)?;
    if status.success() || matches!(status.code(), Some(129 | 130 | 143)) {
        return Ok(());
    }
    bail!("remote launch failed: {status}")
}

pub fn remote_kill(car: &Car) -> Result<()> {
    let installed = installed(car)?;
    let body = render_kill_script(&installed.install_dir);
    let mut ssh = Command::new("ssh");
    ssh.arg(&installed.remote)
        .arg(format!("bash -c {}", shell_single_quote(&body)));
    run(ssh, false)
}

struct Installed {
    remote: String,
    script: String,
    install_dir: String,
}

fn installed(car: &Car) -> Result<Installed> {
    let Some(deploy) = &car.deploy else {
        bail!(
            "car `{}` has no [deploy] section; it is launch-only",
            car.name
        );
    };
    let remote = format!("{}@{}", deploy.user, deploy.host);
    let script = format!("{}/bin/launch", deploy.install_dir);
    let quoted = shell_single_quote(&script);

    let check = Command::new("ssh")
        .arg(&remote)
        .arg(format!("head -n 5 {quoted}"))
        .output()
        .with_context(|| format!("reading {script} on {remote}"))?;
    if !check.status.success() {
        let stderr = String::from_utf8_lossy(&check.stderr);
        let stderr = stderr.trim();
        let detail = if stderr.is_empty() {
            String::new()
        } else {
            format!(": {stderr}")
        };
        bail!(
            "no launch script for {} at {script} (deploy {} first){detail}",
            car.name,
            car.name
        );
    }
    let stdout = String::from_utf8_lossy(&check.stdout);
    if let Some(message) = installed_car_error(&car.name, &script, &stdout) {
        bail!(message);
    }
    Ok(Installed {
        remote,
        script,
        install_dir: deploy.install_dir.clone(),
    })
}

/// Give ssh the terminal, so Ctrl-C is delivered to the remote script instead
/// of killing the local `cargo`/`cev` process group and dropping the session.
fn ssh_in_foreground(mut command: Command) -> Result<ExitStatus> {
    use std::os::unix::process::CommandExt;

    let tty = libc::STDIN_FILENO;
    let previous = unsafe { libc::tcgetpgrp(tty) };
    if previous < 0 {
        let rendered = render(&command);
        println!("[cev] {rendered}");
        return command
            .status()
            .with_context(|| format!("running `{rendered}`"));
    }

    unsafe {
        command.pre_exec(|| {
            let pid = libc::getpid();
            if libc::setpgid(0, pid) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let rendered = render(&command);
    println!("[cev] {rendered}");
    let mut child = command
        .spawn()
        .with_context(|| format!("running `{rendered}`"))?;
    let pid = child.id() as libc::pid_t;
    // Restores the caller's foreground group even if waiting on ssh panics.
    let _restore = RestoreTty { tty, previous };
    unsafe {
        libc::setpgid(pid, pid);
        // Ignore these only in this process, after spawn, so ssh still receives
        // SIGINT and we are not stopped by SIGTTOU while switching groups.
        libc::signal(libc::SIGINT, libc::SIG_IGN);
        libc::signal(libc::SIGTTOU, libc::SIG_IGN);
        libc::tcsetpgrp(tty, pid);
    }
    child
        .wait()
        .with_context(|| format!("waiting for `{rendered}`"))
}

struct RestoreTty {
    tty: i32,
    previous: libc::pid_t,
}

impl Drop for RestoreTty {
    fn drop(&mut self) {
        unsafe {
            libc::signal(libc::SIGTTOU, libc::SIG_IGN);
            libc::tcsetpgrp(self.tty, self.previous);
        }
    }
}

/// `None` when `stdout` is the launch script for `car`.
fn installed_car_error(car: &str, script: &str, stdout: &str) -> Option<String> {
    match script_car_stamp(stdout) {
        Some(name) if name == car => None,
        Some(name) => Some(format!(
            "installed launch is for `{name}`; deploy `{car}` first"
        )),
        None => Some(format!(
            "no launch script for {car} at {script} (deploy {car} first)"
        )),
    }
}

fn install_launch_script(
    car: &Car,
    nodes: &[LaunchNode],
    remote: &str,
    install_dir: &str,
    dry_run: bool,
) -> Result<()> {
    let dest = format!("{install_dir}/bin/launch");
    println!("[cev] launch script {dest}");
    if nodes.is_empty() {
        println!("[cev]   (no cargo nodes)");
    }
    for node in nodes {
        if node.args.is_empty() {
            println!("[cev]   {}: {}", node.name, node.bin);
        } else {
            println!(
                "[cev]   {}: {} {}",
                node.name,
                node.bin,
                node.args.join(" ")
            );
        }
    }

    let script = render_launch_script(&car.name, install_dir, &car.env, nodes);
    if script_car_stamp(&script) != Some(car.name.as_str()) {
        bail!("launch script for `{}` is missing its car stamp", car.name);
    }

    let path = std::env::temp_dir().join(format!("cev-launch-{}-{}", car.name, std::process::id()));
    let result = (|| -> Result<()> {
        std::fs::write(&path, &script).with_context(|| format!("writing {}", path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&path)?.permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms)?;
        }
        let mut rsync = Command::new("rsync");
        rsync.arg("-a").arg(&path).arg(format!("{remote}:{dest}"));
        run(rsync, dry_run)
    })();
    let _ = std::fs::remove_file(&path);
    result
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_a_script_for_another_car() {
        let text = "#!/bin/bash\n# cev-car: teleop\n";
        let wrong = installed_car_error("merceves", "/opt/cev/bin/launch", text).unwrap();
        assert!(wrong.contains("teleop"), "{wrong}");
        assert!(wrong.contains("deploy `merceves`"), "{wrong}");
        assert!(installed_car_error("teleop", "/opt/cev/bin/launch", text).is_none());

        let missing =
            installed_car_error("merceves", "/opt/cev/bin/launch", "#!/bin/bash\n").unwrap();
        assert!(missing.contains("no launch script"), "{missing}");
        assert!(missing.contains("/opt/cev/bin/launch"), "{missing}");
    }
}

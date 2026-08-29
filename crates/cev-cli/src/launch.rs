//! Launching a car's nodes on this machine.

use std::path::Path;
use std::process::{Child, Command};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use cev_config::{Car, Node};

/// How long nodes get to shut down cleanly after Ctrl-C before we kill them.
const GRACE: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(100);

pub fn launch(root: &Path, car: &Car, build: bool) -> Result<()> {
    let nodes: Vec<&Node> = car.enabled_nodes().collect();
    if nodes.is_empty() {
        bail!("car `{}` has no enabled nodes", car.name);
    }

    if build {
        let packages = car.cargo_packages();
        if !packages.is_empty() {
            build_packages(root, &packages, None)?;
        }
    }

    let mut running: Vec<(String, Child)> = Vec::new();
    for node in &nodes {
        let mut command = node_command(root, car, node)?;
        println!("[cev] start {}: {}", node.name, render(&command));
        let child = command
            .spawn()
            .with_context(|| format!("spawning node `{}`", node.name))?;
        running.push((node.name.clone(), child));
    }

    // In a terminal, Ctrl-C reaches the whole foreground process group, so the
    // nodes get SIGINT at the same moment we do. This handler exists so that we
    // stop supervising and start reaping instead of tearing down immediately.
    let interrupted = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&interrupted);
    ctrlc::set_handler(move || flag.store(true, Ordering::SeqCst))
        .context("installing signal handler")?;

    let mut failure = None;
    'supervise: loop {
        if interrupted.load(Ordering::SeqCst) {
            println!("[cev] interrupted, shutting down");
            break;
        }
        for (name, child) in &mut running {
            if let Some(status) = child.try_wait()? {
                // Any node exiting means the stack is no longer the stack, so
                // bring the rest down rather than limping along.
                println!("[cev] {name} exited: {status}");
                if !status.success() {
                    failure = Some(format!("node `{name}` exited with {status}"));
                }
                break 'supervise;
            }
        }
        std::thread::sleep(POLL);
    }

    shutdown(&mut running)?;

    match failure {
        Some(message) => bail!(message),
        None => Ok(()),
    }
}

/// Wait out the grace period, then kill whatever is left.
fn shutdown(running: &mut [(String, Child)]) -> Result<()> {
    let deadline = Instant::now() + GRACE;
    loop {
        let mut alive = 0;
        for (_, child) in running.iter_mut() {
            if child.try_wait()?.is_none() {
                alive += 1;
            }
        }
        if alive == 0 || Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(POLL);
    }
    for (name, child) in running.iter_mut() {
        if child.try_wait()?.is_none() {
            println!("[cev] {name} did not exit within {GRACE:?}, killing");
            let _ = child.kill();
            let _ = child.wait();
        }
    }
    Ok(())
}

/// `cargo build --release` for the given packages, optionally cross-compiling.
pub fn build_packages(root: &Path, packages: &[&str], target: Option<(&str, &str)>) -> Result<()> {
    let (program, triple) = match target {
        Some((builder, triple)) => (builder, Some(triple)),
        None => ("cargo", None),
    };
    let mut command = Command::new(program);
    command.current_dir(root).arg("build").arg("--release");
    if let Some(triple) = triple {
        command.arg("--target").arg(triple);
    }
    for package in packages {
        command.arg("--package").arg(package);
    }
    println!("[cev] {}", render(&command));
    let status = command
        .status()
        .with_context(|| format!("running `{program}` (is it installed and on PATH?)"))?;
    if !status.success() {
        bail!("build failed: {status}");
    }
    Ok(())
}

fn node_command(root: &Path, car: &Car, node: &Node) -> Result<Command> {
    let mut command = match (node.bin_name(), node.external_argv()) {
        (Some(bin), _) => {
            let path = root.join("target/release").join(bin);
            if !path.is_file() {
                bail!(
                    "node `{}`: {} not built (run without --no-build)",
                    node.name,
                    path.display()
                );
            }
            let mut command = Command::new(path);
            command.args(&node.args);
            command
        }
        (None, Some(argv)) => {
            let mut command = Command::new(&argv[0]);
            command.args(&argv[1..]);
            command
        }
        (None, None) => unreachable!("validated at load time"),
    };
    command.current_dir(root);
    command.env("CEV_CAR", &car.name);
    command.env("CEV_CAR_DIR", &car.dir);
    for (key, value) in &car.env {
        command.env(key, value);
    }
    for (key, value) in &node.env {
        command.env(key, value);
    }
    Ok(command)
}

pub fn render(command: &Command) -> String {
    let mut parts = vec![command.get_program().to_string_lossy().into_owned()];
    parts.extend(command.get_args().map(|a| a.to_string_lossy().into_owned()));
    parts.join(" ")
}

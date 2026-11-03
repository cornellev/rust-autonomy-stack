//! `cev` -- orchestration for the CEV autonomy stack.
//!
//! The nodes live in their own repositories, pulled in by `gitman` and built as
//! members of this workspace. The cars live in `cars/`. This binary is the glue:
//! it reads a car manifest and either runs the stack here, ships it, or starts
//! the copy already on the car.

mod deploy;
mod group;
mod launch;
mod ui;

use std::process::ExitCode;

use anyhow::{Context, Result};
use cev_config::Car;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "cev", about = "Launch and deploy the autonomy stack", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List the cars and the nodes each one runs.
    Cars,
    /// Validate car manifests and the state of the workspace.
    Check {
        /// Only check this car.
        car: Option<String>,
    },
    /// Build and run a car's nodes on this machine.
    Launch {
        car: String,
        /// Run whatever is already in target/release.
        #[arg(long)]
        no_build: bool,
    },
    /// Move every node to the tip of its tracked revision and re-lock.
    Pull,
    /// Cross-compile a car's nodes and rsync them onto the vehicle.
    Deploy {
        car: String,
        /// Ship whatever is already built for the car's target.
        #[arg(long)]
        no_build: bool,
        /// Print every command without running any of them.
        #[arg(long)]
        dry_run: bool,
        /// Cross-compile for the car and stop before ssh and rsync.
        #[arg(long, conflicts_with = "no_build")]
        pre_build: bool,
    },
    /// Run the launch script already installed on the car.
    Remote {
        #[command(subcommand)]
        command: RemoteCommand,
    },
}

#[derive(Subcommand)]
enum RemoteCommand {
    /// Start the deployed stack on the car and wait until it stops.
    Launch { car: String },
    /// Stop the deployed stack on the car if it is running.
    Kill { car: String },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            // `{e:#}` prints the whole anyhow chain on one line: "a: b: c".
            ui::error(format!("{e:#}"));
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    let root = group::cwd_root()?;
    let cars_dir = root.join("cars");

    match cli.command {
        Command::Cars => cars(&root, &cars_dir),
        Command::Check { car } => check(&root, &cars_dir, car.as_deref()),
        Command::Pull => pull(&root),
        Command::Launch { car, no_build } => {
            let car = Car::load(&cars_dir, &car)?;
            launch::launch(&root, &car, !no_build)
        }
        Command::Deploy {
            car,
            no_build,
            dry_run,
            pre_build,
        } => {
            let car = Car::load(&cars_dir, &car)?;
            deploy::deploy(&root, &car, !no_build, dry_run, pre_build)
        }
        Command::Remote { command } => match command {
            RemoteCommand::Launch { car } => {
                let car = Car::load(&cars_dir, &car)?;
                deploy::remote_launch(&car)
            }
            RemoteCommand::Kill { car } => {
                let car = Car::load(&cars_dir, &car)?;
                deploy::remote_kill(&car)
            }
        },
    }
}

/// The logo, then every car and the nodes it runs. Same list as before.
fn cars(root: &std::path::Path, cars_dir: &std::path::Path) -> Result<()> {
    let packages = group::packages(root)?;
    let present: Vec<&str> = packages.iter().map(|pkg| pkg.name.as_str()).collect();
    let names = Car::list(cars_dir)?;
    ui::banner(&format!("autonomy stack · {} car(s)", names.len()));

    for name in names {
        let car = Car::load(cars_dir, &name)?;
        anstream::println!("  {}  {}", ui::car_badge(&car.name), car.description);
        for node in &car.nodes {
            let mut how = match (node.bin_name(), node.external_argv()) {
                (Some(bin), _) => format!("cargo {bin}"),
                (None, Some(argv)) => argv.join(" "),
                (None, None) => String::new(),
            };
            if let Some(profile) = &node.profile {
                how.push_str(&format!(" ({profile})"));
            }
            let skipped = node.optional
                && node
                    .package
                    .as_deref()
                    .is_some_and(|package| !present.contains(&package));
            if skipped {
                anstream::println!(
                    "     {}",
                    ui::dim(format!("○ {:<12} {how} (skipped)", node.name))
                );
            } else if node.enabled {
                anstream::println!(
                    "     {} {} {}",
                    ui::accent("●"),
                    ui::strong(format!("{:<12}", node.name)),
                    how
                );
            } else {
                anstream::println!("     {}", ui::dim(format!("○ {:<12} {how}", node.name)));
            }
        }
        anstream::println!();
    }
    Ok(())
}

/// `uvx gitman update` from the group root: tip of each source `rev`, then rewrite
/// `sources_locked`. Dirty clones abort the update; this does not pass `--force`.
fn pull(root: &std::path::Path) -> Result<()> {
    let mut command = std::process::Command::new("uvx");
    command.arg("gitman").arg("update").current_dir(root);
    let rendered = launch::render(&command);
    println!("[cev] {rendered}");
    let status = command
        .status()
        .with_context(|| format!("running `{rendered}` (is uv installed?)"))?;
    if !status.success() {
        anyhow::bail!("`{rendered}` failed: {status}");
    }
    println!("[cev] gitman.yml now pins the new commits; commit it to record the stack version");
    Ok(())
}

fn check(root: &std::path::Path, cars_dir: &std::path::Path, only: Option<&str>) -> Result<()> {
    let mut problems = 0;

    for path in group::broken_links(root)? {
        ui::error(format!(
            "broken: {} is a dangling link (rerun `uvx gitman install`)",
            path.display()
        ));
        problems += 1;
    }

    let packages = group::packages(root)?;
    let names: Vec<&str> = packages.iter().map(|p| p.name.as_str()).collect();
    for package in &packages {
        let dir = package.dir.strip_prefix(root).unwrap_or(&package.dir);
        ui::status("Package", format!("{:<16} {}", package.name, dir.display()));
    }

    let cars = match only {
        Some(name) => vec![name.to_string()],
        None => Car::list(cars_dir)?,
    };
    for name in cars {
        let car = Car::load(cars_dir, &name)?;
        ui::status(
            "Checking",
            format!("car {name}: {} node(s)", car.nodes.len()),
        );
        for node in car.enabled_nodes() {
            let Some(package) = node.package.as_deref() else {
                continue;
            };
            if names.contains(&package) {
                continue;
            }
            if node.optional {
                ui::status("Skipped", format!("{} (no package {package})", node.name));
                continue;
            }
            ui::error(format!(
                "missing: node package `{package}` is not in the workspace \
                 (is it in gitman.yml, and did you run `uvx gitman install`?)"
            ));
            problems += 1;
        }
        for node in &car.nodes {
            if node.profile.is_none() {
                continue;
            }
            let Some(package) = node.package.as_deref() else {
                continue;
            };
            if !names.contains(&package) {
                continue;
            }
            if let Err(err) = launch::node_args(node, &packages) {
                ui::error(format!("{err:#}"));
                problems += 1;
            }
        }
    }

    if problems == 0 {
        ui::status("Finished", "ok");
        Ok(())
    } else {
        anyhow::bail!("{problems} problem(s)")
    }
}

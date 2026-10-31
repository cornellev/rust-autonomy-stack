//! `cev` -- orchestration for the CEV autonomy stack.
//!
//! The nodes live in their own repositories, pulled in by `gitman` and built as
//! members of this workspace. The cars live in `cars/`. This binary is the glue:
//! it reads a car manifest and either runs the stack here or ships it there.

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
    },
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
        Command::Cars => cars(&cars_dir),
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
        } => {
            let car = Car::load(&cars_dir, &car)?;
            deploy::deploy(&root, &car, !no_build, dry_run)
        }
    }
}

/// The logo, then every car and the nodes it runs. Same list as before.
fn cars(cars_dir: &std::path::Path) -> Result<()> {
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
            if node.enabled {
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
        for package in car.cargo_packages() {
            if !names.contains(&package) {
                ui::error(format!(
                    "missing: node package `{package}` is not in the workspace \
                     (is it in gitman.yml, and did you run `uvx gitman install`?)"
                ));
                problems += 1;
            }
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

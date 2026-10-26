//! `cev` -- orchestration for the CEV autonomy stack.
//!
//! The nodes live in their own repositories, pulled in as git dependencies of
//! `crates/cev-nodes` and built with `cargo build -p <node>`.  The cars live in `cars/`. This
//! binary is the glue:
//! it reads a car manifest and either runs the stack here or ships it there.
//WARNING: this should all build together into one systemctl service that can be started by the
//user, not per package.

mod deploy;
mod group;
mod launch;
mod ui;

use std::process::ExitCode;

use anyhow::Result;
use cev_config::Car;
use cev_config::contract::NodeContract;
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

/// The logo, then every car: its kind, modes and GPU, and the nodes it runs.
fn cars(cars_dir: &std::path::Path) -> Result<()> {
    let names = Car::list(cars_dir)?;
    ui::banner(&format!("autonomy stack · {} car(s)", names.len()));

    for name in names {
        let car = Car::load(cars_dir, &name)?;
        let modes: Vec<&str> = car.modes.iter().map(|m| m.name()).collect();
        let gpu = if car.gpu { "gpu" } else { "cpu only" };
        anstream::println!(
            "  {}  {}",
            ui::car_badge(&car.name),
            ui::dim(format!(
                "{} · {} · {gpu}",
                car.kind.name(),
                modes.join(", ")
            ))
        );
        if !car.description.is_empty() {
            anstream::println!("     {}", car.description);
        }
        for node in &car.nodes {
            let how = match (node.bin_name(), node.external_argv()) {
                (Some(bin), _) => bin.to_string(),
                (None, Some(argv)) => argv.join(" "),
                (None, None) => String::new(),
            };
            if node.enabled {
                anstream::println!(
                    "     {} {} {}",
                    ui::accent("●"),
                    ui::strong(format!("{:<12}", node.name)),
                    how
                );
            } else {
                anstream::println!(
                    "     {}",
                    ui::dim(format!("○ {:<12} {how} (disabled)", node.name))
                );
            }
        }
        anstream::println!();
    }
    Ok(())
}

fn check(root: &std::path::Path, cars_dir: &std::path::Path, only: Option<&str>) -> Result<()> {
    let mut problems = 0;
    let packages = group::packages(root)?;

    let cars = match only {
        Some(name) => vec![name.to_string()],
        None => Car::list(cars_dir)?,
    };
    for name in cars {
        let car = Car::load(cars_dir, &name)?;
        ui::status(
            "Checking",
            format!("car {name} ({} node(s))", car.nodes.len()),
        );
        for wanted in car.cargo_packages() {
            match packages.iter().find(|p| p.name == wanted) {
                Some(package) => match NodeContract::load(&package.contract_path()) {
                    Ok(contract) => {
                        ui::status("Found", format!("{wanted} {}", contract.path.display()))
                    }
                    Err(e) => {
                        ui::error(e);
                        problems += 1;
                    }
                },
                None => {
                    ui::error(format!(
                        "node package `{wanted}` is not resolved \
                    (is it a dependency in crates/cev-nodes/Cargo.toml?)"
                    ));
                    problems += 1;
                }
            }
        }
    }

    if problems == 0 {
        ui::status("Finished", "no problems");
        Ok(())
    } else {
        anyhow::bail!("{problems} problem(s)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every package node on every car ships a valid contract that names
    /// the package that it lives in.
    #[test]
    fn every_car_node_has_valid_contract() {
        let root = group::find_root(std::path::Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap();
        let packages = group::packages(&root).unwrap();
        let cars_dir = root.join("cars");

        for name in Car::list(&cars_dir).unwrap() {
            let car = Car::load(&cars_dir, &name).unwrap();
            for wanted in car.cargo_packages() {
                let Some(package) = packages.iter().find(|p| p.name == wanted) else {
                    panic!("car {name}: node package `{wanted}` is not resolved");
                };
                let contract = match NodeContract::load(&package.contract_path()) {
                    Ok(contract) => contract,
                    Err(e) => panic!("car {name}: {e}"),
                };
                assert_eq!(
                    contract.run.package.as_deref(),
                    Some(wanted),
                    "car {name}: {} displays the wrong package",
                    contract.path.display()
                );
            }
        }
    }
}

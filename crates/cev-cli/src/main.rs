//! `cev` -- orchestration for the CEV autonomy stack.
//!
//! The nodes live in their own repositories, pulled in by `gitman` and built as
//! members of this workspace. The cars live in `cars/`. This binary is the glue:
//! it reads a car manifest and either runs the stack here or ships it there.

mod deploy;
mod group;
mod launch;

use anyhow::Result;
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

fn main() -> Result<()> {
    let cli = Cli::parse();
    let root = group::cwd_root()?;
    let cars_dir = root.join("cars");

    match cli.command {
        Command::Cars => {
            for name in Car::list(&cars_dir)? {
                let car = Car::load(&cars_dir, &name)?;
                println!("{name}  {}", car.description);
                for node in &car.nodes {
                    let how = match (node.bin_name(), node.external_argv()) {
                        (Some(bin), _) => format!("cargo {bin}"),
                        (None, Some(argv)) => argv.join(" "),
                        (None, None) => String::new(),
                    };
                    let mark = if node.enabled { " " } else { "-" };
                    println!("  {mark} {:<12} {how}", node.name);
                }
            }
            Ok(())
        }
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

fn check(root: &std::path::Path, cars_dir: &std::path::Path, only: Option<&str>) -> Result<()> {
    let mut problems = 0;

    for path in group::broken_links(root)? {
        println!(
            "broken: {} is a dangling link (rerun `uvx gitman install`)",
            path.display()
        );
        problems += 1;
    }

    let packages = group::packages(root)?;
    let names: Vec<&str> = packages.iter().map(|p| p.name.as_str()).collect();
    for package in &packages {
        let dir = package.dir.strip_prefix(root).unwrap_or(&package.dir);
        println!("package {:<16} {}", package.name, dir.display());
    }

    let cars = match only {
        Some(name) => vec![name.to_string()],
        None => Car::list(cars_dir)?,
    };
    for name in cars {
        let car = Car::load(cars_dir, &name)?;
        println!("car {name}: {} node(s)", car.nodes.len());
        for package in car.cargo_packages() {
            if !names.contains(&package) {
                println!(
                    "  missing: node package `{package}` is not in the workspace \
                     (is it in gitman.yml, and did you run `uvx gitman install`?)"
                );
                problems += 1;
            }
        }
    }

    if problems == 0 {
        println!("ok");
        Ok(())
    } else {
        anyhow::bail!("{problems} problem(s)")
    }
}

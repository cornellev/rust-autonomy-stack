//! `cev` -- orchestration for the CEV autonomy stack.
//!
//! The nodes live in their own repositories, pulled in by `gitman` and built as
//! members of this workspace. The cars live in `cars/`. This binary is the glue:
//! it reads a car manifest and either runs the stack here or ships it there.

mod deploy;
mod group;
mod launch;

use anyhow::Result;
use cev_config::{Car, Catalog};
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
    /// Show the sensor catalog, or what one car carries.
    Sensors {
        /// Car to show. Omit to list the catalog itself.
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
        Command::Sensors { car } => sensors(&root, &cars_dir, car.as_deref()),
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
    // Loading the catalog validates every model file: id-vs-stem, and each
    // hardware kind's own physical bounds.
    let catalog = Catalog::load(&root.join("sensors"))?;
    println!("catalog: {} sensor model(s)", catalog.len());
    let mut unmeasured = 0;

    for name in cars {
        let car = Car::load(cars_dir, &name)?;
        println!(
            "car {name}: {} node(s), {} sensor(s)",
            car.nodes.len(),
            car.sensors.len()
        );
        for package in car.cargo_packages() {
            if !names.contains(&package) {
                println!(
                    "  missing: node package `{package}` is not in the workspace \
                     (is it in gitman.yml, and did you run `uvx gitman install`?)"
                );
                problems += 1;
            }
        }
        // resolve_sensors is what catches an unknown model id and a rate override
        // above the model's native rate.
        match car.resolve_sensors(&catalog) {
            Ok(sensors) => {
                for sensor in sensors {
                    if let Err(error) = sensor.extrinsics() {
                        println!("  bad pose: {error}");
                        problems += 1;
                    }
                    if sensor.model.is_datasheet() {
                        unmeasured += 1;
                    }
                }
            }
            Err(error) => {
                println!("  {error}");
                problems += 1;
            }
        }
    }

    // Not a failure -- a datasheet sigma is a real number, just not one measured
    // on our hardware. It is worth saying out loud before it becomes an R matrix.
    if unmeasured > 0 {
        println!(
            "note: {unmeasured} registered sensor(s) use unmeasured (datasheet) noise; \
             run an Allan/FFT fit before trusting them in a filter"
        );
    }

    if problems == 0 {
        println!("ok");
        Ok(())
    } else {
        anyhow::bail!("{problems} problem(s)")
    }
}

/// `cev sensors [car]` -- the catalog, or one car's registrations resolved against it.
fn sensors(root: &std::path::Path, cars_dir: &std::path::Path, car: Option<&str>) -> Result<()> {
    let catalog = Catalog::load(&root.join("sensors"))?;

    let Some(name) = car else {
        if catalog.is_empty() {
            println!("no sensor models in {}", root.join("sensors").display());
            return Ok(());
        }
        for model in catalog.models() {
            let measured = if model.is_datasheet() { "!" } else { " " };
            println!(
                "{measured} {:<12} {:<14} {:>8.1} Hz  {}",
                model.id,
                model.kind_name(),
                model.rate_hz,
                model.description
            );
        }
        println!("\n! = noise is unmeasured (datasheet or absent)");
        return Ok(());
    };

    let car = Car::load(cars_dir, name)?;
    let sensors = car.resolve_sensors(&catalog)?;
    if sensors.is_empty() {
        println!("{name} registers no sensors");
        return Ok(());
    }

    println!(
        "   {:<16} {:<12} {:<10} {:>9}  key",
        "sensor", "model", "kind", "rate"
    );
    for sensor in &sensors {
        let enabled = if sensor.reg.enabled { " " } else { "-" };
        let measured = if sensor.model.is_datasheet() {
            "!"
        } else {
            " "
        };
        println!(
            "{enabled}{measured} {:<16} {:<12} {:<10} {:>6.1} Hz  {}",
            sensor.reg.name,
            sensor.model.id,
            sensor.model.kind_name(),
            sensor.rate_hz(),
            sensor.key_expr(&car.name),
        );
        let extrinsics = sensor.extrinsics()?;
        let [roll, pitch, yaw] = extrinsics.rpy();
        println!(
            "     {} <- xyz [{:.3}, {:.3}, {:.3}] m  rpy [{:.3}, {:.3}, {:.3}] rad",
            extrinsics.parent,
            extrinsics.xyz[0],
            extrinsics.xyz[1],
            extrinsics.xyz[2],
            roll,
            pitch,
            yaw
        );
    }
    println!("\n- = disabled, ! = noise is unmeasured (datasheet or absent)");
    Ok(())
}

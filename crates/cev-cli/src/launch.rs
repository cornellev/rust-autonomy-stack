//! Launching a car's nodes on this machine, and the script that does it on the car.

use std::collections::BTreeMap;
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
    let workspace = crate::group::packages(root)?;
    let present: Vec<&str> = workspace.iter().map(|pkg| pkg.name.as_str()).collect();
    let nodes = car.runnable(&present);
    if nodes.is_empty() {
        bail!("car `{}` has no enabled nodes", car.name);
    }

    let planned: Vec<Vec<String>> = nodes
        .iter()
        .map(|node| node_args(node, &workspace))
        .collect::<Result<_>>()?;

    if build {
        build_car(root, &nodes, &workspace, None, false)?;
    }

    let mut running: Vec<(String, Child)> = Vec::new();
    for (node, args) in nodes.iter().zip(&planned) {
        let mut command = node_command(root, car, node, args)?;
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

/// Build the cargo nodes that will actually run.
///
/// Workspace members go through `cargo build -p`. A package whose manifest has
/// a `[workspace]` table is built by manifest path into the group `target/`
/// directory, so launch and deploy still find the binary there.
pub fn build_car(
    root: &Path,
    nodes: &[&Node],
    workspace: &[crate::group::Package],
    target: Option<(&str, &str)>,
    dry_run: bool,
) -> Result<()> {
    let mut members: Vec<&str> = Vec::new();
    let mut detached: Vec<&crate::group::Package> = Vec::new();
    for node in nodes {
        let Some(package) = node.package.as_deref() else {
            continue;
        };
        let Some(found) = workspace.iter().find(|pkg| pkg.name == package) else {
            continue;
        };
        if found.detached {
            if !detached.iter().any(|pkg| pkg.name == found.name) {
                detached.push(found);
            }
        } else if !members.contains(&package) {
            members.push(package);
        }
    }

    if dry_run {
        let mut names = members;
        names.extend(detached.iter().map(|pkg| pkg.name.as_str()));
        if !names.is_empty() {
            match target {
                Some((_, triple)) => {
                    println!("[cev] would build {} for {triple}", names.join(", "))
                }
                None => println!("[cev] would build {}", names.join(", ")),
            }
        }
        return Ok(());
    }

    if !members.is_empty() {
        build_packages(root, &members, target)?;
    }
    for pkg in detached {
        build_detached(root, &pkg.dir.join("Cargo.toml"), target)?;
    }
    Ok(())
}

/// `cargo build --release` for workspace members, optionally cross-compiling.
fn build_packages(root: &Path, packages: &[&str], target: Option<(&str, &str)>) -> Result<()> {
    let (program, triple) = target_program(target);
    let mut command = Command::new(program);
    command.current_dir(root).arg("build").arg("--release");
    if let Some(triple) = triple {
        command.arg("--target").arg(triple);
    }
    for package in packages {
        command.arg("--package").arg(package);
    }
    execute(program, &mut command)
}

fn build_detached(root: &Path, manifest: &Path, target: Option<(&str, &str)>) -> Result<()> {
    let (program, triple) = target_program(target);
    let mut command = Command::new(program);
    command
        .current_dir(root)
        .arg("build")
        .arg("--release")
        .arg("--manifest-path")
        .arg(manifest)
        .arg("--target-dir")
        .arg(root.join("target"));
    if let Some(triple) = triple {
        command.arg("--target").arg(triple);
    }
    execute(program, &mut command)
}

fn target_program<'a>(target: Option<(&'a str, &'a str)>) -> (&'a str, Option<&'a str>) {
    match target {
        Some((builder, triple)) => (builder, Some(triple)),
        None => ("cargo", None),
    }
}

fn execute(program: &str, command: &mut Command) -> Result<()> {
    println!("[cev] {}", render(command));
    let status = command
        .status()
        .with_context(|| format!("running `{program}` (is it installed and on PATH?)"))?;
    if !status.success() {
        bail!("build failed: {status}");
    }
    Ok(())
}

/// Profile args, then the node's own `args`. No profile means `args` only.
pub fn node_args(node: &Node, packages: &[crate::group::Package]) -> Result<Vec<String>> {
    let mut args = Vec::new();
    if let Some(name) = &node.profile {
        let Some(package) = node.package.as_deref() else {
            bail!("node `{}` sets `profile` without `package`", node.name);
        };
        let dir = packages
            .iter()
            .find(|pkg| pkg.name == package)
            .map(|pkg| &pkg.dir)
            .with_context(|| {
                format!(
                    "node `{}`: package `{package}` is not in the workspace \
                     (is it in gitman.yml, and did you run `uvx gitman install`?)",
                    node.name
                )
            })?;
        args.extend(
            cev_config::profile_args(dir, name).with_context(|| format!("node `{}`", node.name))?,
        );
    }
    args.extend(node.args.iter().cloned());
    Ok(args)
}

fn node_command(root: &Path, car: &Car, node: &Node, args: &[String]) -> Result<Command> {
    let mut command = if let Some(bin) = node.bin_name() {
        let path = root.join("target/release").join(bin);
        if !path.is_file() {
            bail!(
                "node `{}`: {} not built (run without --no-build)",
                node.name,
                path.display()
            );
        }
        Command::new(path)
    } else if let Some(program) = node.command.first() {
        let mut command = Command::new(program);
        command.args(&node.command[1..]);
        command
    } else {
        unreachable!("validated at load time")
    };
    command.args(args);
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

/// One cargo node, with profile args already resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchNode {
    pub name: String,
    pub bin: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
}

/// Enabled cargo nodes in manifest order. External `command` nodes are omitted.
pub fn launch_nodes(car: &Car, packages: &[crate::group::Package]) -> Result<Vec<LaunchNode>> {
    let present: Vec<&str> = packages.iter().map(|pkg| pkg.name.as_str()).collect();
    let mut nodes = Vec::new();
    for node in car.runnable(&present) {
        let Some(bin) = node.bin_name() else {
            continue;
        };
        nodes.push(LaunchNode {
            name: node.name.clone(),
            bin: bin.to_string(),
            args: node_args(node, packages)?,
            env: node.env.clone(),
        });
    }
    Ok(nodes)
}

/// Car name from a `# cev-car:` line. Empty input and other text have none.
pub fn script_car_stamp(text: &str) -> Option<&str> {
    text.lines()
        .find_map(|line| line.strip_prefix("# cev-car: "))
        .map(str::trim)
        .filter(|name| !name.is_empty())
}

pub fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

fn export_line(key: &str, value: &str) -> String {
    format!(
        "export {}={}",
        shell_single_quote(key),
        shell_single_quote(value)
    )
}

/// Bash that runs `nodes` from the directory containing the script.
///
/// Shebang has to be the first line, so the car stamp is the second line.
/// `install_dir` is recorded in a comment; the script finds `bin/` and `config/`
/// from its own path. `flock` on fd 9 is what holds the single-instance lock
/// open for the life of the shell.
pub fn render_launch_script(
    car_name: &str,
    install_dir: &str,
    car_env: &BTreeMap<String, String>,
    nodes: &[LaunchNode],
) -> String {
    let mut script = format!("#!/bin/bash\n# cev-car: {car_name}\n# install: {install_dir}\n");
    if nodes.is_empty() {
        script.push_str("echo \"launch: no nodes\" >&2\nexit 1\n");
        return script;
    }

    script.push_str(
        "BIN=$(CDPATH= cd -- \"$(dirname \"$0\")\" && pwd)\n\
         CONFIG=$(CDPATH= cd -- \"$BIN/..\" && pwd)/config\n",
    );
    script.push_str(&format!(
        "export CEV_CAR={}\nexport CEV_CAR_DIR=\"$CONFIG\"\n",
        shell_single_quote(car_name)
    ));
    for (key, value) in car_env {
        script.push_str(&export_line(key, value));
        script.push('\n');
    }
    script.push('\n');
    script.push_str(&supervisor(nodes));
    script
}

fn supervisor(nodes: &[LaunchNode]) -> String {
    let mut seen = Vec::new();
    let mut bins = String::new();
    for node in nodes {
        if seen.contains(&node.bin.as_str()) {
            continue;
        }
        seen.push(node.bin.as_str());
        if !bins.is_empty() {
            bins.push(' ');
        }
        bins.push_str(&shell_single_quote(&node.bin));
    }

    // Pieces stay raw bash. Only the bin list, the grace seconds, and the
    // start blocks are inserted, so braces in the script are not format syntax.
    let mut script = String::from(
        r#"# Background jobs ignore SIGINT unless monitor mode is on.
set -m
if ! command -v flock >/dev/null 2>&1; then
  echo "launch: flock is required" >&2
  exit 1
fi
exec 9>"$BIN/launch.lock"
if ! flock -n 9; then
  echo "launch: stack already running (lock $BIN/launch.lock)" >&2
  exit 1
fi

missing=0
for bin in "#,
    );
    script.push_str(&bins);
    script.push_str(
        r#"; do
  if [[ ! -x "$BIN/$bin" ]]; then
    echo "launch: missing $BIN/$bin" >&2
    missing=1
  fi
done
if [[ "$missing" -ne 0 ]]; then
  exit 1
fi

pids=()
names=()

stop() {
  local code=$1
  trap '' INT TERM HUP
  local i=0
  local alive
  while [[ "$i" -lt "${#pids[@]}" ]]; do
    # set -m makes each node its own process group. Signaling the pid alone
    # misses grandchildren; the group id is the node's pid.
    kill -INT -- "-${pids[$i]}" 2>/dev/null || true
    kill -INT "${pids[$i]}" 2>/dev/null || true
    i=$((i + 1))
  done
  local deadline=$((SECONDS + "#,
    );
    script.push_str(&GRACE.as_secs().to_string());
    script.push_str(
        r#"))
  while [[ "$SECONDS" -lt "$deadline" ]]; do
    alive=0
    i=0
    while [[ "$i" -lt "${#pids[@]}" ]]; do
      if kill -0 "${pids[$i]}" 2>/dev/null; then
        alive=1
      fi
      i=$((i + 1))
    done
    if [[ "$alive" -eq 0 ]]; then
      exit "$code"
    fi
    sleep 0.1
  done
  i=0
  while [[ "$i" -lt "${#pids[@]}" ]]; do
    kill -KILL -- "-${pids[$i]}" 2>/dev/null || true
    kill -KILL "${pids[$i]}" 2>/dev/null || true
    i=$((i + 1))
  done
  exit "$code"
}

trap 'stop 130' INT
trap 'stop 143' TERM
trap 'stop 129' HUP
echo $$ > "$BIN/launch.pid"
trap 'rm -f "$BIN/launch.pid"' EXIT

"#,
    );
    for node in nodes {
        script.push_str(&start_block(node));
    }
    script.push_str(
        r#"while true; do
  i=0
  while [[ "$i" -lt "${#pids[@]}" ]]; do
    if ! kill -0 "${pids[$i]}" 2>/dev/null; then
      wait "${pids[$i]}"
      status=$?
      echo "launch: ${names[$i]} exited: $status" >&2
      stop "$status"
    fi
    i=$((i + 1))
  done
  sleep 0.1
done
"#,
    );
    script
}

fn start_block(node: &LaunchNode) -> String {
    let mut block = format!("# {}\n(\n  exec 9>&-\n", node.name);
    for (key, value) in &node.env {
        block.push_str("  ");
        block.push_str(&export_line(key, value));
        block.push('\n');
    }
    block.push_str("  cd \"$CONFIG\" || exit 1\n  exec \"$BIN\"/");
    block.push_str(&shell_single_quote(&node.bin));
    for arg in &node.args {
        block.push(' ');
        block.push_str(&shell_single_quote(arg));
    }
    block.push_str(&format!(
        "\n) &\npids+=(\"$!\")\nnames+=({})\n",
        shell_single_quote(&node.name)
    ));
    block
}

/// Stop the stack recorded in `<install_dir>/bin/launch.pid`.
///
/// Prints `launch: not running` and exits 0 when nothing is alive. Otherwise
/// SIGINT the supervisor and its children, then SIGKILL after the same grace
/// `stop` uses.
pub fn render_kill_script(install_dir: &str) -> String {
    let pidfile = shell_single_quote(&format!("{install_dir}/bin/launch.pid"));
    let mut script = format!("pidfile={pidfile}\n");
    script.push_str(
        r#"if [[ ! -f "$pidfile" ]]; then
  echo "launch: not running" >&2
  exit 0
fi
pid=$(cat "$pidfile")
if ! kill -0 "$pid" 2>/dev/null; then
  echo "launch: not running" >&2
  exit 0
fi
kids=$(pgrep -P "$pid" 2>/dev/null || true)
signal_tree() {
  local sig=$1
  local p pgid
  for p in $pid $kids; do
    pgid=$(ps -o pgid= -p "$p" 2>/dev/null | tr -d ' ')
    if [[ -n "$pgid" ]]; then
      kill "$sig" -- "-$pgid" 2>/dev/null || true
    fi
    kill "$sig" "$p" 2>/dev/null || true
  done
}
signal_tree -INT
deadline=$((SECONDS + "#,
    );
    script.push_str(&GRACE.as_secs().to_string());
    script.push_str(
        r#"))
while [[ "$SECONDS" -lt "$deadline" ]]; do
  alive=0
  for p in $pid $kids; do
    if kill -0 "$p" 2>/dev/null; then
      alive=1
    fi
  done
  if [[ "$alive" -eq 0 ]]; then
    exit 0
  fi
  sleep 0.1
done
signal_tree -KILL
exit 0
"#,
    );
    script
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;
    use std::process::Stdio;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn quotes_stamp_env_and_args() {
        assert_eq!(shell_single_quote("it's"), "'it'\\''s'");
        let mut car_env = BTreeMap::new();
        car_env.insert("RUST_LOG".to_string(), "info".to_string());
        let mut node_env = BTreeMap::new();
        node_env.insert("MSG".to_string(), "it's fine".to_string());
        let nodes = vec![LaunchNode {
            name: "perception".to_string(),
            bin: "zenoh_wrapper".to_string(),
            args: vec!["--note".to_string(), "a b".to_string()],
            env: node_env,
        }];
        let script = render_launch_script("merceves", "/opt/cev", &car_env, &nodes);

        let mut lines = script.lines();
        assert_eq!(lines.next(), Some("#!/bin/bash"));
        assert_eq!(lines.next(), Some("# cev-car: merceves"));
        assert_eq!(lines.next(), Some("# install: /opt/cev"));
        assert!(script.contains("export CEV_CAR='merceves'\nexport CEV_CAR_DIR=\"$CONFIG\"\n"));
        assert!(
            script.find("export CEV_CAR_DIR").unwrap()
                < script.find("export 'RUST_LOG'='info'").unwrap()
        );
        assert!(script.contains(&export_line("MSG", "it's fine")));
        assert!(script.contains("exec \"$BIN\"/'zenoh_wrapper' '--note' 'a b'\n"));
        assert!(script.contains("flock -n 9"));
        assert!(script.contains(&format!("SECONDS + {}", GRACE.as_secs())));
        assert_eq!(script_car_stamp(&script), Some("merceves"));
        assert_eq!(script_car_stamp("no stamp\n"), None);
        bash_syntax(&script);
    }

    #[test]
    fn empty_stack_exits() {
        let script = render_launch_script("merceves", "/opt/cev", &BTreeMap::new(), &[]);
        assert!(script.contains("launch: no nodes"));
        assert_eq!(script_car_stamp(&script), Some("merceves"));
        bash_syntax(&script);
    }

    #[test]
    fn launch_nodes_follow_the_manifest() {
        let tmp = Scratch::new("nodes");
        let cars = tmp.path.join("cars");
        let car_dir = cars.join("merceves");
        std::fs::create_dir_all(&car_dir).unwrap();
        std::fs::write(
            car_dir.join("car.toml"),
            r#"
name = "merceves"

[env]
RUST_LOG = "info"

[[nodes]]
name = "teleop"
package = "teleop"
profile = "dry-run"
args = ["--extra"]

[[nodes]]
name = "planning"
package = "costmap_planner_rs"
bin = "a_star"
args = ["--map", "track"]

[[nodes]]
name = "imu-gps"
package = "imu-gps-node"
enabled = false

[[nodes]]
name = "planner"
command = ["ros2", "run", "pkg", "node"]

[[nodes]]
name = "deadreckoning"
package = "cev_odometry"
optional = true
"#,
        )
        .unwrap();
        write_package(&tmp.path.join("nodes/teleop"), "teleop");
        std::fs::write(
            tmp.path.join("nodes/teleop/profiles.toml"),
            "[dry-run]\nargs = [\"--dry-run\"]\n",
        )
        .unwrap();
        write_package(&tmp.path.join("nodes/planning"), "costmap_planner_rs");

        let car = Car::load(&cars, "merceves").unwrap();
        let packages = crate::group::packages(&tmp.path).unwrap();
        let nodes = launch_nodes(&car, &packages).unwrap();
        assert_eq!(
            nodes
                .iter()
                .map(|node| node.name.as_str())
                .collect::<Vec<_>>(),
            ["teleop", "planning"]
        );
        assert_eq!(nodes[0].bin, "teleop");
        assert_eq!(nodes[0].args, ["--dry-run", "--extra"]);
        assert_eq!(nodes[1].bin, "a_star");
        assert_eq!(nodes[1].args, ["--map", "track"]);

        let script = render_launch_script(&car.name, "/opt/cev", &car.env, &nodes);
        assert!(script.contains("export 'RUST_LOG'='info'"));
        assert!(script.contains("exec \"$BIN\"/'teleop' '--dry-run' '--extra'\n"));
        assert!(script.contains("exec \"$BIN\"/'a_star' '--map' 'track'\n"));
        let teleop_at = script.find("teleop").unwrap();
        assert!(teleop_at < script.find("a_star").unwrap());
        assert!(!script.contains("imu-gps"));
        assert!(!script.contains("ros2"));
        assert!(!script.contains("cev_odometry"));
        bash_syntax(&script);
    }

    #[test]
    fn missing_binary_starts_nothing() {
        let tmp = Scratch::new("missing");
        let bin = tmp.path.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(tmp.path.join("config")).unwrap();
        let sentinel = tmp.path.join("started");
        write_exe(
            &bin.join("other"),
            &format!(
                "#!/bin/bash\ntouch {}\n",
                shell_single_quote(&sentinel.display().to_string())
            ),
        );
        let nodes = vec![
            LaunchNode {
                name: "other".to_string(),
                bin: "other".to_string(),
                args: Vec::new(),
                env: BTreeMap::new(),
            },
            LaunchNode {
                name: "gone".to_string(),
                bin: "gone".to_string(),
                args: Vec::new(),
                env: BTreeMap::new(),
            },
        ];
        let script = render_launch_script("merceves", "/opt/cev", &BTreeMap::new(), &nodes);
        let launch = write_script(&bin.join("launch"), &script);
        let mut session = Session::spawn(&launch, &flock_stub(&tmp.path.join("stub")));
        let status = session.wait_timeout(Duration::from_secs(3));
        assert!(!status.success(), "{}", session.output());
        assert!(
            !sentinel.exists(),
            "started a node before every binary was present"
        );
    }

    #[test]
    fn one_exit_reaps_the_others() {
        let tmp = Scratch::new("reap");
        let bin = tmp.path.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(tmp.path.join("config")).unwrap();
        let pidfile = tmp.path.join("sleeper.pid");
        let pid_arg = pidfile.display().to_string();
        write_exe(
            &bin.join("sleeper"),
            "#!/bin/bash\necho $$ > \"$1\"\nexec sleep 30\n",
        );
        write_exe(
            &bin.join("dier"),
            "#!/bin/bash\ni=0\nwhile [[ \"$i\" -lt 50 ]]; do\n  if [[ -s \"$1\" ]]; then exit 1; fi\n  sleep 0.05\n  i=$((i + 1))\ndone\nexit 1\n",
        );
        let nodes = vec![
            LaunchNode {
                name: "sleeper".to_string(),
                bin: "sleeper".to_string(),
                args: vec![pid_arg.clone()],
                env: BTreeMap::new(),
            },
            LaunchNode {
                name: "dier".to_string(),
                bin: "dier".to_string(),
                args: vec![pid_arg],
                env: BTreeMap::new(),
            },
        ];
        let script = render_launch_script("merceves", "/opt/cev", &BTreeMap::new(), &nodes);
        let launch = write_script(&bin.join("launch"), &script);
        let mut session = Session::spawn(&launch, &flock_stub(&tmp.path.join("stub")));
        // SIGINT should reap the sleeper. The 5s KILL fallback is not this test;
        // a 3s cap fails the run if children are still ignoring SIGINT.
        let status = session.wait_timeout(Duration::from_secs(3));
        assert!(!status.success(), "{}", session.output());
        let pid = std::fs::read_to_string(&pidfile)
            .unwrap_or_else(|_| panic!("sleeper never wrote its pid\n{}", session.output()));
        let alive = Command::new("kill")
            .args(["-0", pid.trim()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success();
        assert!(!alive, "sleeper {pid} still running\n{}", session.output());
    }

    #[test]
    fn interrupt_and_hangup_stop_the_stack() {
        for sig in ["-INT", "-HUP"] {
            let mut running = Running::start(&format!("sig{}", sig.trim_start_matches('-')));
            let launch_pid =
                std::fs::read_to_string(running.tmp.path.join("bin/launch.pid")).unwrap();
            let signaled = Command::new("kill")
                .args([sig, launch_pid.trim()])
                .status()
                .unwrap();
            assert!(signaled.success(), "kill {sig}");
            let _ = running.session.wait_timeout(Duration::from_secs(3));
            let sleeper = std::fs::read_to_string(&running.sleeper_pid).unwrap();
            assert!(
                !process_alive(&sleeper),
                "sleeper {sleeper} still running after {sig}\n{}",
                running.session.output()
            );
        }
    }

    #[test]
    fn kill_script_stops_a_running_stack() {
        let mut running = Running::start("kill");
        let body = render_kill_script(&running.tmp.path.display().to_string());
        let output = Command::new("bash").arg("-c").arg(&body).output().unwrap();
        assert!(
            output.status.success(),
            "kill script: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let _ = running.session.wait_timeout(Duration::from_secs(3));
        let sleeper = std::fs::read_to_string(&running.sleeper_pid).unwrap();
        assert!(
            !process_alive(&sleeper),
            "sleeper {sleeper} still running\n{}\n{}",
            running.session.output(),
            String::from_utf8_lossy(&output.stderr)
        );

        let again = Command::new("bash").arg("-c").arg(&body).output().unwrap();
        let stderr = String::from_utf8_lossy(&again.stderr);
        assert!(again.status.success(), "{stderr}");
        assert!(stderr.contains("not running"), "{stderr}");
    }

    fn process_alive(pid: &str) -> bool {
        Command::new("kill")
            .args(["-0", pid.trim()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success()
    }

    struct Running {
        tmp: Scratch,
        session: Session,
        sleeper_pid: PathBuf,
    }

    impl Running {
        fn start(label: &str) -> Self {
            let tmp = Scratch::new(label);
            let bin = tmp.path.join("bin");
            std::fs::create_dir_all(&bin).unwrap();
            std::fs::create_dir_all(tmp.path.join("config")).unwrap();
            let sleeper_pid = tmp.path.join("sleeper.pid");
            write_exe(
                &bin.join("sleeper"),
                "#!/bin/bash\necho $$ > \"$1\"\nexec sleep 30\n",
            );
            let nodes = vec![LaunchNode {
                name: "sleeper".to_string(),
                bin: "sleeper".to_string(),
                args: vec![sleeper_pid.display().to_string()],
                env: BTreeMap::new(),
            }];
            let script = render_launch_script(
                "merceves",
                &tmp.path.display().to_string(),
                &BTreeMap::new(),
                &nodes,
            );
            let launch = write_script(&bin.join("launch"), &script);
            let session = Session::spawn(&launch, &flock_stub(&tmp.path.join("stub")));
            let mut running = Self {
                tmp,
                session,
                sleeper_pid,
            };
            running.wait_until_up();
            running
        }

        fn wait_until_up(&mut self) {
            let launch_pid = self.tmp.path.join("bin/launch.pid");
            let start = Instant::now();
            loop {
                if self.session.poll().is_some() {
                    panic!("script exited early\n{}", self.session.output());
                }
                let sleeper = std::fs::read_to_string(&self.sleeper_pid).unwrap_or_default();
                let launch = std::fs::read_to_string(&launch_pid).unwrap_or_default();
                if !sleeper.trim().is_empty() && !launch.trim().is_empty() {
                    return;
                }
                if start.elapsed() > Duration::from_secs(3) {
                    self.session.kill_group();
                    panic!("stack did not come up\n{}", self.session.output());
                }
                std::thread::sleep(POLL);
            }
        }
    }

    impl Drop for Running {
        fn drop(&mut self) {
            if let Ok(pid) = std::fs::read_to_string(&self.sleeper_pid) {
                let _ = Command::new("kill")
                    .args(["-KILL", pid.trim()])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
        }
    }

    fn bash_syntax(script: &str) {
        let tmp = Scratch::new("syntax");
        let path = write_script(&tmp.path.join("launch"), script);
        let output = Command::new("bash").arg("-n").arg(&path).output().unwrap();
        assert!(
            output.status.success(),
            "bash -n: {}\n{script}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn write_package(dir: &Path, name: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join("Cargo.toml"),
            format!("[package]\nname = \"{name}\"\n"),
        )
        .unwrap();
    }

    fn write_exe(path: &Path, body: &str) {
        std::fs::write(path, body).unwrap();
        mark_executable(path);
    }

    fn write_script(path: &Path, body: &str) -> std::path::PathBuf {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, body).unwrap();
        mark_executable(path);
        path.to_path_buf()
    }

    fn mark_executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).unwrap();
    }

    // macOS has no flock(1). The stub checks the invocation; util-linux flock
    // on the car holds fd 9 open for the shell.
    fn flock_stub(dir: &Path) -> std::path::PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join("flock");
        write_exe(
            &path,
            "#!/bin/bash\n[[ \"$1\" == \"-n\" && \"$2\" == \"9\" ]]\n",
        );
        dir.to_path_buf()
    }

    struct Scratch {
        path: std::path::PathBuf,
    }

    impl Scratch {
        fn new(label: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir()
                .join(format!("cev-launch-{label}-{}-{nanos}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    struct Session {
        child: Child,
        pgid: u32,
        exited: Option<std::process::ExitStatus>,
        stdout: Option<std::process::ChildStdout>,
        stderr: Option<std::process::ChildStderr>,
    }

    impl Session {
        fn spawn(script: &Path, stub: &Path) -> Self {
            use std::os::unix::process::CommandExt;
            let path = std::env::var("PATH").unwrap_or_default();
            let mut command = Command::new(script);
            command
                .env("PATH", format!("{}:{path}", stub.display()))
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .process_group(0);
            let mut child = command.spawn().unwrap();
            let pgid = child.id();
            Self {
                stdout: child.stdout.take(),
                stderr: child.stderr.take(),
                child,
                pgid,
                exited: None,
            }
        }

        fn poll(&mut self) -> Option<std::process::ExitStatus> {
            if self.exited.is_none() {
                if let Some(status) = self.child.try_wait().unwrap() {
                    self.exited = Some(status);
                }
            }
            self.exited
        }

        fn wait_timeout(&mut self, limit: Duration) -> std::process::ExitStatus {
            let start = Instant::now();
            loop {
                if let Some(status) = self.poll() {
                    return status;
                }
                if start.elapsed() > limit {
                    self.kill_group();
                    panic!("launch script did not exit\n{}", self.output());
                }
                std::thread::sleep(POLL);
            }
        }

        fn output(&mut self) -> String {
            use std::io::Read;
            let mut out = String::new();
            if let Some(stdout) = self.stdout.as_mut() {
                let mut buf = String::new();
                let _ = stdout.read_to_string(&mut buf);
                out.push_str(&buf);
            }
            if let Some(stderr) = self.stderr.as_mut() {
                let mut buf = String::new();
                let _ = stderr.read_to_string(&mut buf);
                out.push_str(&buf);
            }
            out
        }

        fn kill_group(&mut self) {
            let _ = Command::new("kill")
                .args(["-KILL", &format!("-{}", self.pgid)])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            let _ = self.child.wait();
        }
    }

    impl Drop for Session {
        fn drop(&mut self) {
            self.kill_group();
        }
    }
}

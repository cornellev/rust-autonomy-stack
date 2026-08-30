# Running the stack

## Setup

You need Rust (1.85+, edition 2024), `uv`, `git`, `rsync` and `ssh`.

```bash
gh auth setup-git            # once: several node repos are private
git clone https://github.com/cornellev/rust-autonomy-stack
cd rust-autonomy-stack
uvx gitman install           # clone every node at its locked commit
cargo cev check              # validate the workspace and every car
cargo build                  # build everything
```

`gitman install` creates `repos/` (the clones) and `nodes/` (symlinks to the
Cargo packages inside them). Both are untracked; the group repo only tracks the
manifest that produces them.

## Everyday commands

```bash
cargo cars                       # what cars exist and what nodes they run
cargo cev check                  # dangling links, missing packages, bad manifests
cargo cev sensors                # the sensor catalog
cargo cev sensors merceves       # what one car carries, with poses and zenoh keys
cargo launch sim                 # build + run the stack here, Ctrl-C to stop
cargo launch merceves --no-build # run what is already in target/release
cargo deploy merceves --dry-run  # print every command without running it
cargo deploy merceves            # cross-compile, rsync to the car, restart
```

`cargo deploy` and friends are aliases in `.cargo/config.toml` for
`cargo run -p cev-cli -- <subcommand>`; `cev` works from anywhere inside the
group, including from inside a node clone.

## Adding a sensor

1. Add `sensors/<id>.toml` to the catalog -- one file per sensor *model*, and `id`
   must equal the file stem. Pick the `kind` and fill in that kind's parameter
   block; the fields differ per hardware and unknown ones are a parse error, so
   the error message will list what is expected.
2. Fill in `[model.params.noise]` and a `[calibration]` block saying where the
   numbers came from. If they are off a vendor sheet, say `method = "datasheet"`
   -- `cev check` reports unmeasured noise, which is the prompt to run an Allan or
   FFT fit before those numbers become an estimator's `R`.
3. Register it in `cars/<car>/sensors.toml` with an instance name, a `parent`
   frame, and a pose. `xyz` is metres and `rpy` is radians as intrinsic Z-Y-X;
   the transform is sensor-to-parent, `v_parent = R * v_sensor + xyz`. Use
   `quat = [w, x, y, z]` instead if the pose came from a calibration tool.
4. `cargo cev check`, then `cargo cev sensors <car>` to see it resolved.

The catalog is car-agnostic on purpose: it is the registry of parts the team owns,
and the same noise numbers feed both the simulator and the estimator so the two
cannot drift apart. Per-vehicle facts -- which units are fitted, and where -- stay
in the car's `sensors.toml`.

## Working on a node

The clone in `repos/<name>` is a normal git repository. Branch, commit and push
there as usual:

```bash
cd repos/rust-inekf
git checkout -b feature/imu-bias
# ...
git push -u origin feature/imu-bias
```

Builds pick your working tree up immediately -- `nodes/estimation` is a symlink
to that same directory, so `cargo build` at the group root compiles what you just
edited. Bulk git operations across every clone:

```bash
.tools/git_recursive.sh fetch --all
.tools/git_recursive.sh status -sb
.tools/git_recursive_reset_branch.sh          # reset everything to default branches
.tools/git_recursive_delete_merged_branches.sh
```

## Moving the pinned revisions

`gitman.yml` holds both what we track (`sources`, usually `rev: main`) and the
exact commits (`sources_locked`). They do different jobs:

```bash
uvx gitman install     # check out sources_locked -- the reproducible stack
uvx gitman update      # move to the tip of each rev and rewrite sources_locked
uvx gitman lock        # freeze whatever is currently checked out
uvx gitman list        # what is checked out right now
```

Commit `gitman.yml` after `update` or `lock`: that diff *is* the stack version
bump, and it is what tells you what a given car was running.

## Gotchas

- **`gitman install` leaves a detached HEAD.** It checks out a locked SHA, so
  after running it your branch is no longer checked out in that clone. The branch
  and its commits are untouched -- `git checkout feature/imu-bias` picks up right
  where you left off.
- **Uncommitted changes block gitman**, by design. It refuses with
  `Uncommitted changes in repos/<name>` rather than discarding them. Commit, or
  pass `--skip-changes` to leave that dependency alone. `--force` *does* discard.
- **`gitman` rewrites `gitman.yml` and eats your comments.** `lock` and `update`
  round-trip the file: comments above the *first* source survive, everything
  below is deleted, and every source grows empty `params:`/`scripts:`/`patches:`
  keys. Keep per-source notes in `README.md`.
- **Workspace-wide commands reach into the node clones.** `cargo fmt --all` and
  `cargo clippy --fix` will happily rewrite `repos/<node>`, since those really
  are workspace members. Scope them: `cargo fmt -p cev-cli -p cev-config`. Each
  node repo owns its own style.
- **`nodes/` is tracked but empty** (a `.gitkeep`). Cargo treats `nodes/*` as a
  literal path when the directory does not exist, so a fresh clone would fail
  every cargo command before `gitman install` ran. Do not delete the directory.
- **Only Cargo packages may be linked into `nodes/`.** A linked directory with no
  `Cargo.toml` fails the `nodes/*` workspace glob and breaks the build for
  everyone, not just the person who added it. `cargo cev check` catches the
  dangling-link case; the non-package case is on you at review time.
- **A node that is itself a Cargo workspace cannot be a member.** Cargo does not
  nest workspaces. Link the individual packages inside it instead of the repo
  root, or keep it out of the workspace and launch it as a `command` node.
- **The `.gitignore` is a whitelist.** Everything is ignored unless explicitly
  unignored, which is what keeps the nested clones from polluting group-level git
  operations. If you add a new top-level file and git ignores it, add a `!` line.
- **`cargo deploy` needs `cross`** unless the car sets `builder = "cargo"`:
  `cargo install cross` (it needs Docker or Podman). With `builder = "cargo"` you
  supply the cross linker yourself via `rustup target add <triple>` and a
  `[target.<triple>]` linker entry in `.cargo/config.toml`.
- **Deploy does not ship non-Cargo nodes.** The ROS 2 planner is built by colcon
  in its own environment; `cev deploy` prints a note and skips it.
- **A new top-level directory needs a `.gitignore` entry.** The whitelist ignores
  everything by default so that gitman's clones stay invisible, so `sensors/` is
  tracked only because there is an explicit `!sensors/**` line. Anything new needs
  the same, or it will look committed locally and be missing for everyone else.
- **Do not commit `Cargo.lock` from an incomplete checkout.** If you run any cargo
  command before `uvx gitman install`, `nodes/*` is empty, so cargo prunes every
  node's dependencies -- zenoh, tokio, nalgebra and the rest -- out of the lock
  file. It looks like a tidy diff and it silently un-pins the stack. Run
  `uvx gitman install` first; if you already did it the other way round,
  `git checkout Cargo.lock`, install, and build again.
- **A sensor's `id` must equal its filename**, the same rule `car.toml` follows for
  `name` and its directory. `cev check` catches a mismatch.

## What deploy actually does

For each enabled node with a `package`:

1. `cross build --release --target <triple> -p <package>` (or `cargo`),
2. `ssh <user>@<host> mkdir -p <install_dir>/{bin,config,sensors}`,
3. `rsync` the binaries to `<install_dir>/bin/`,
4. `rsync --delete cars/<car>/ ` to `<install_dir>/config/`,
5. `rsync --delete sensors/ ` to `<install_dir>/sensors/`,
6. `ssh <user>@<host> <restart_cmd>` if the car sets one.

Steps 4 and 5 are separate destinations on purpose. The sensor catalog is
group-level rather than per-car, so it is not inside `cars/<car>/`; and it cannot
be dropped into `config/` either, because step 4 runs with `--delete` and would
remove it again. Nodes are launched with `CEV_CAR_DIR` and `CEV_SENSORS_DIR`
pointing at the two directories, locally and on the car alike.

Every command is printed before it runs, and `--dry-run` prints them all without
running any. There is no supervision on the car yet: `restart_cmd` is where a
systemd unit or similar hooks in.

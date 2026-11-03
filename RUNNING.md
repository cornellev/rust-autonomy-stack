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
cargo cars                        # what cars exist and what nodes they run
cargo cev check                   # dangling links, missing packages, bad manifests
cargo launch sim                  # build + run the stack here, Ctrl-C to stop
cargo launch merceves --no-build  # run what is already in target/release
cargo deploy merceves --dry-run   # print every command without running it
cargo deploy merceves --pre-build # cross-compile only; do not ssh or rsync
cargo deploy merceves             # cross-compile, rsync binaries and bin/launch
cargo cev remote launch merceves  # ssh to the car and run <install_dir>/bin/launch
cargo cev remote kill merceves    # stop that stack if it is still running
cargo pull                        # tip of each node's tracked rev, then re-lock
```

`cargo deploy` and friends are aliases in `.cargo/config.toml` for
`cargo run -p cev-cli -- <subcommand>`; `cev` works from anywhere inside the
group, including from inside a node clone.

## October 31 closed loop

The simulator page has to be open on `oct-31-testing` before the bridge starts,
because the bridge refuses to run until the orchestrator has the sim's message
schemas. With the orchestrator and the sim dev server already up:

```bash
scripts/oct31_closed_loop.sh
```

That starts the oct-31 bridge, `cargo launch sim`, and `scripts/fake_goal.py`.
Start the episode in the browser after the stack prints its nodes. Pass looks
like this: the global grid updates about once a second and lines up with the
map, A* publishes trajectories, the car stops within 1 m of the goal 10 m
ahead, and `cev/localization/pose` stays close to `cev/sim/truth/pose`. If the
Zenoh peers never see each other, start `zenohd` on this machine and rerun.

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
edited.

`nodes/deadreckoning` is local-only. It is gitignored, excluded from the workspace,
and not a gitman source. Cars that set `optional = true` on `cev_odometry` build and
run it when that directory exists, and skip it when it does not. Do not add it to
`gitman.yml`.

Bulk git operations across every clone:

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
cargo pull             # same as `uvx gitman update`
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
- **`imu-gps-node` does not compile on macOS.** `linux-embedded-hal` is Linux-only, so a plain `cargo build` at the group root fails here. Deploy still builds it: `cross` targets `aarch64-unknown-linux-gnu`. Other packages: `cargo build --workspace --exclude imu-gps-node`.
- **`cargo deploy` needs `cross`** unless the car sets `builder = "cargo"`:
  `cargo install cross` (it needs Docker or Podman). With `builder = "cargo"` you
  supply the cross linker yourself via `rustup target add <triple>` and a
  `[target.<triple>]` linker entry in `.cargo/config.toml`.
- **Deploy does not ship non-Cargo nodes.** The ROS 2 planner is built by colcon
  in its own environment; `cev deploy` prints a note and skips it.

## What deploy actually does

For each enabled node with a `package`:

1. `cross build --release --target <triple> -p <package>` (or `cargo`),
2. `ssh <user>@<host> mkdir -p <install_dir>/{bin,config}`,
3. `rsync` the binaries to `<install_dir>/bin/`,
4. `rsync` `<install_dir>/bin/launch`, the script that starts the enabled cargo nodes with profile args baked in,
5. `rsync --delete cars/<car>/ ` to `<install_dir>/config/`,
6. `ssh <user>@<host> <restart_cmd>` if the car sets one.

Every command is printed before it runs, and `--dry-run` prints them all without
running any. `--pre-build` stops after the compile, once each release binary is
on disk; `--dry-run --pre-build` only prints the build line. Deploy installs the
launch script and does not start it. `cargo cev remote launch <car>` reads the
`# cev-car:` stamp, then runs the script in the foreground over `ssh -t`.
Ctrl-C and a dropped connection stop the nodes, not only the ssh session.
`cargo cev remote kill <car>` does the same when you are no longer attached.
On the car, run
`<install_dir>/bin/launch` (`/opt/cev/bin/launch` for MerCEVes and teleop). One
node exiting stops the rest, and a second start exits while that stack holds
the lock. Those two cars share `/opt/cev`, so the stamp has to match the car
you name; deploy that car again to switch. The car needs `bash` and `flock`.
`restart_cmd` still runs afterward when the manifest sets one.

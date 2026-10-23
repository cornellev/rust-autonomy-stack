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
cargo launch sim                 # build + run the stack here, Ctrl-C to stop
cargo launch merceves --no-build # run what is already in target/release
cargo deploy merceves --dry-run  # print every command without running it
cargo deploy merceves            # cross-compile, rsync to the car, restart
```

`cargo deploy` and friends are aliases in `.cargo/config.toml` for
`cargo run -p cev-cli -- <subcommand>`; `cev` works from anywhere inside the
group, including from inside a node clone.

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
- **Some nodes need SDKs your laptop may not have.** `zed-reader` panics in
  `build.rs` without the ZED SDK (`/usr/local/zed`, or set `ZED_SDK_DIR`), and
  `virtual-rgbd-sensor`'s auto-discovered `src/bin/wrapper.rs` uses `rclrs`
  without declaring it, so it fails outside a ROS 2 setup. To build everything else:
  ```bash
  cargo build --workspace --exclude zed-reader --exclude virtual-rgbd-sensor
  cargo build -p virtual-rgbd-sensor --bin rslidar_viz --bin zenoh_viz  # its working bins
  ```
  `--exclude` only works together with `--workspace`. `cargo launch`/`deploy`
  build only the packages a car enables, so this matters only for bare `cargo build`.
- **Some nodes only build on the car / in a ROS 2 env.** `zed-reader`'s
  `build.rs` panics without the ZED SDK + CUDA (`ZED_SDK_DIR`, default
  `/usr/local/zed`), and `virtual-rgbd-sensor` (`nodes/perception`) needs
  `rclrs` and the ROS message crates from a sourced ROS 2 workspace. On a dev
  machine without them, exclude them:
  `cargo build --workspace --exclude zed-reader --exclude virtual-rgbd-sensor`.
  `--exclude` takes the *package* name, not the `nodes/` directory name.
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
4. `rsync --delete cars/<car>/ ` to `<install_dir>/config/`,
5. `ssh <user>@<host> <restart_cmd>` if the car sets one.

Every command is printed before it runs, and `--dry-run` prints them all without
running any. There is no supervision on the car yet: `restart_cmd` is where a
systemd unit or similar hooks in.

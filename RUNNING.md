# Running the stack

## Setup

You need Rust (1.85+, edition 2024), `git`, `rsync` and `ssh`.

```bash
gh auth setup-git            # once: lets Cargo fetch private node repos over https
git clone https://github.com/cornellev/rust-autonomy-stack
cd rust-autonomy-stack
cargo cev check              # fetches every node at its locked commit, validates every car
```

The first cargo command fetches every node repository at the commit in
`Cargo.lock` into `~/.cargo/git`. Nothing is cloned into this directory.

## Everyday commands

```bash
cargo cars                       # what cars exist and what nodes they run
cargo cev check                  # every car's nodes resolve, manifests are valid
cargo launch sim                 # build the car's nodes + run them here, Ctrl-C to stop
cargo launch merceves --no-build # run what is already in target/release
cargo deploy merceves --dry-run  # print every command without running it
cargo deploy merceves            # cross-compile, rsync to the car, restart
cargo build --release -p holdover-ekf   # build one node by hand
```

`cargo deploy` and friends are aliases in `.cargo/config.toml` for
`cargo run -p cev-cli -- <subcommand>`; `cev` works from anywhere inside this
repository.

## Working on a node

Work in your own clone of the node, next to this one:

```
rust-autonomy/
├── .cargo/config.toml       your local patches   (not in any repo)
├── rust-autonomy-stack/
├── holdover-ekf/
└── rust-autonomy-sensors/
```

To make the stack build your clone instead of the pinned commit, add a
`[patch]` to `rust-autonomy/.cargo/config.toml`, in the directory *above* the
stack. Cargo reads config files from every parent directory, so this applies
here without touching any tracked file:

```toml
[patch."https://github.com/cornellev/holdover-ekf"]
holdover-ekf = { path = "holdover-ekf" }

[patch."https://github.com/cornellev/rust-autonomy-sensors"]
sensor-defs = { path = "rust-autonomy-sensors/crates/sensor-defs" }
zed-reader = { path = "rust-autonomy-sensors/crates/zed-reader" }
```

Paths are relative to `rust-autonomy/`. Every build under that directory picks
the patch up, including standalone builds of `holdover-ekf` (which then use
your local `sensor-defs` too). Remove or comment out a section to go back to the
pinned commit.

Branch, commit and push in the node's clone as usual. To move the stack onto
your pushed work:

```bash
cargo update -p holdover-ekf     # move to the tip of its branch; commit Cargo.lock
```

To track a different branch, change `branch = "..."` for that node in
`crates/cev-nodes/Cargo.toml`.

## Gotchas

- **Never commit `Cargo.lock` while a local patch is active.** The patch
  rewrites the lock entry to your local path. Comment the patch out, run
  `cargo update -p <node>`, and commit that lock instead.
- **A node's path dependency on another repo breaks the whole stack.** Inside a
  git checkout, `path = "../rust-autonomy-sensors/..."` points at nothing, and
  every cargo command here, `cargo cev` included, fails with
  `no matching package named ...`. Node repos depend on each other by `git`.
- **Keep the same `git` URL and `branch` for a repo everywhere.** Cargo treats
  `branch = "extra-sensors"` and `rev = "5d892c1..."` as two different sources,
  even when they are the same commit, and builds two copies of `sensor-defs`.
- **Bin-only node packages are silently ignored.** Cargo drops a git dependency
  that has no library target with only a warning, and `cargo build -p` then
  says the package does not exist. Give the node an `src/lib.rs`.
- **Some nodes only build on the car.** `zed-reader`'s `build.rs` needs the ZED
  SDK and CUDA (`ZED_SDK_DIR`, default `/usr/local/zed`). Bare `cargo build`,
  `test` and `clippy` skip all nodes (`cev-nodes` is not a default member), and
  `cev launch`/`deploy` build only the car's nodes. Avoid `--workspace` on a
  laptop.
- **Workspace-wide `cargo fmt` cannot reach the nodes any more** (they are not
  members), so `cargo fmt --all` is safe again.
- **`cargo deploy` needs `cross`** unless the car sets `builder = "cargo"`:
  `cargo install cross` (it needs Docker or Podman). With `builder = "cargo"` you
  supply the cross linker yourself via `rustup target add <triple>` and a
  `[target.<triple>]` linker entry in `.cargo/config.toml`.
- **Deploy does not ship `command` nodes.** A node a car runs as
  `command = [...]` is built and installed by its own toolchain; `cev deploy`
  prints a note and skips it.

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

# rust-autonomy-stack

Master repository for the full autonomous stack for our cars.

Nothing that runs on a car lives here. This repository is *glue*: it names the
node repositories, pins the exact commit of each one, builds them as one Cargo
graph, and holds one manifest per car describing what that car runs and where
it runs. Plain Cargo does all of the fetching, pinning and building; there is no
second tool.

```
rust-autonomy-stack/
├── Cargo.toml         the workspace
├── Cargo.lock         the exact commit of every node   (commit it: this is the release)
├── cars/              one directory per vehicle -- the deployment manifests
│   ├── merceves/car.toml
│   └── sim/car.toml
└── crates/
    ├── cev-cli/       the `cev` binary: cars, check, launch, deploy
    ├── cev-config/    the car.toml and cev-node.toml schemas
    └── cev-nodes/     which node repos make up the stack, and at which branch
```

## How nodes get in

Each node is its own git repository. `crates/cev-nodes/Cargo.toml` lists them as
git dependencies:

```toml
holdover-ekf = { git = "https://github.com/cornellev/holdover-ekf", branch = "sensor-integration" }
```

`cev-nodes` itself is an empty library; it exists so that Cargo resolves, fetches
and locks every node. From there:

- **Pinning.** `Cargo.lock` records the commit each `branch` resolved to.
  Committing `Cargo.lock` freezes a stack release; `cargo update -p <node>` moves
  one node to the tip of its branch, and that lock diff *is* the version bump.
- **Building.** `cargo build --release -p <node>` builds a node's binary into
  `target/release/`, even though the node is a dependency rather than a member.
  `cev launch` and `cev deploy` do exactly that for the nodes a car names.
- **Finding contracts.** `cargo metadata` reports where each node's checkout is
  (under `~/.cargo/git`), which is where `cev` reads its `cev-node.toml`.
- **Editing.** Point a node at your own clone with a `[patch]`; see
  [RUNNING.md](RUNNING.md#working-on-a-node).

`cev-nodes` is not a default workspace member, so a bare `cargo build`, `cargo
test` or `cargo clippy` compiles only the orchestrator, never the nodes (some
of which need SDKs a laptop does not have).

## What a node repository must provide

- **A Cargo package with a library target**, even an empty `src/lib.rs`. Cargo
  silently ignores a git dependency with no lib, so a bin-only package cannot be
  built with `-p`.
- **No path dependencies outside its own repository.** Inside a git checkout,
  `path = "../other-repo"` points at nothing, and the whole stack then fails to
  resolve. Depend on other node repos by `git`, with the same URL and `branch`
  the stack uses so Cargo unifies them into one copy.
- **A `cev-node.toml` contract** next to its `Cargo.toml`: topics in and out,
  message types, rates, how to run, CPU or GPU, modes, metrics. The schema and
  its rules live in `crates/cev-config/src/contract.rs`. Topics are Zenoh keys
  (`sensors/imu`, never `/sensors/imu`, no wildcards); message types are names
  from `sensor_defs::MsgType`.

A repository that is itself a Cargo workspace (like `rust-autonomy-sensors`) is
fine: the git dependency names the package, and Cargo finds it inside.

## Cars live here, not in a per-car repository

The alternative was a separate deployment repository per car, selected when you
clone. We keep cars **in-tree** instead, and selection moves from clone time to
run time:

```
cargo launch sim              # run the stack on this machine
cargo deploy merceves         # cross-compile, rsync, restart on the car
```

The reasoning: a car manifest is small declarative data whose schema lives in
`crates/cev-config` here, so a schema change and the config change it forces are
one commit rather than a two-repo dance; `Cargo.lock` and the car manifest
together *are* the release, and splitting them across repositories gives you two
things to keep in sync and no single place that says what a car is running. One
clone gets a new team member a full workspace.

Open question: per-car *hardware* constants -- wheelbase, track, wheel radius,
sensor extrinsics, CAN IDs -- are not deployment truth and are deliberately
absent from `car.toml`. They are needed by estimation, control and perception
alike, so they want one home rather than three copies. Nothing owns them yet;
the candidates are a `hardware.toml` beside `car.toml` with a schema in
`cev-config`, or a crate of their own.

## The nodes

| package | repository | branch | role | in a car? |
| --- | --- | --- | --- | --- |
| `holdover-ekf` | `holdover-ekf` | `sensor-integration` | state estimation | yes, all cars |
| `rust-inekf` | `rust-inekf` | `main` | state estimation | not currently |
| `zed-reader` | `rust-autonomy-sensors` | `extra-sensors` | ZED camera | not yet |

`sensor-defs` (message types) comes from `rust-autonomy-sensors` on the same
branch. `virtual-rgbd-sensor` is left out until it has a library target. The
planner is not in the stack yet: the current `costmap-planner-rust` is ROS 2,
and it comes in once it is converted to a Zenoh-native Cargo package.

## Adding a node

1. Make sure the repo meets the requirements above.
2. Add a line to `crates/cev-nodes/Cargo.toml` and a row to the table above.
3. Add a `[[nodes]]` entry to the cars that should run it.
4. `cargo cev check`, then commit `Cargo.toml`, `Cargo.lock` and the car.

See [RUNNING.md](RUNNING.md) for setup and day-to-day commands.

# rust-autonomy-stack

Master repository for the full autonomous stack for our cars.

Nothing that runs on a car lives here. This repository is *glue*: it names the
node repositories, pins the exact commit of each one, ties them into a single
Cargo workspace, and holds one manifest per car describing what that car runs and
where it runs. Modelled on
[ihmcrobotics/repository-group](https://github.com/ihmcrobotics/repository-group),
with `gitman` in place of manual clones and a Cargo workspace in place of the
composite Gradle build.

```
rust-autonomy-stack/
├── gitman.yml         which node repos, at which commits   (sources + sources_locked)
├── Cargo.toml         the workspace that builds all of them
├── cars/              one directory per vehicle
│   ├── merceves/
│   │   ├── car.toml      what it runs and where        (deployment truth)
│   │   └── sensors.toml  what it carries and where     (hardware truth)
│   └── sim/
├── sensors/           car-agnostic catalog: one file per sensor model we own
├── crates/            first-party code that belongs to the group itself
│   ├── cev-cli/       the `cev` binary: cars, check, launch, deploy
│   └── cev-config/    the car.toml schema
├── .tools/            recursive git helpers across every cloned repo
├── repos/             gitman clones land here          (untracked)
└── nodes/             symlinks to the Cargo packages   (contents untracked, workspace members)
```

## The two views of a node

`gitman` clones into `repos/<repo-name>` -- ordinary git repositories with
branches and remotes, which is where you actually work. It then symlinks the
Cargo package inside each one to `nodes/<role>`, and the workspace globs over
`nodes/*`. So `repos/` is organised by repository and `nodes/` by role, and both
point at the same files.

Two things fall out of this that are worth knowing:

- `exclude = ["repos"]` in the root `Cargo.toml` means `cd repos/rust-inekf &&
  cargo test` still works standalone, while `nodes/estimation` is a member of the
  group workspace. One checkout, two valid build contexts.
- Only link a directory that *is* a Cargo package. A `nodes/` entry without a
  `Cargo.toml` breaks the workspace glob for everyone. That is why
  `costmap-planner-rust` (ROS 2 / `rclrs`, built by colcon) is pulled by gitman
  but not linked; a car runs it as a `command = [...]` node instead.

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
one commit rather than a two-repo dance; `sources_locked` in `gitman.yml` and the
car manifest together *are* the release, and splitting them across repositories
gives you two things to keep in sync and no single place that says what a car is
running. One clone gets a new team member a full workspace.

This is cheap to reverse. If a car's config outgrows the group -- a subteam owns
it, or it carries secrets -- add a gitman source with
`links: - target: cars/<name>` and nothing else changes; there is a commented
stanza in `gitman.yml` showing exactly that.

## Sensors: a catalog, and what each car registers from it

Hardware truth is split across two files, along the line that actually matters:
whether a fact belongs to the *part* or to the *vehicle*.

`sensors/<id>.toml` is the **catalog** -- one file per sensor model the team owns,
independent of any car. An entry is common metadata (vendor, native rate) plus
exactly one kind-specific parameter block. Those blocks do not share a shape,
because the hardware does not: a camera carries resolution, intrinsics, distortion
and a shutter model; an IMU carries measurement ranges and bias random walks.
Putting a camera's `fx` in an IMU file is a parse error, not a silently ignored key.

The catalog exists for **sim-real fidelity**. Its noise parameters -- measured by
Allan variance or FFT analysis of bench data -- are read twice: the simulator
corrupts ground truth with them, and the estimator builds its measurement
covariance `R` from them. They are one number so those two cannot drift apart. A
`[calibration]` block records where each number came from, and `method =
"datasheet"` marks one that was *not* measured on our hardware; `cev check` says
so out loud, because a vendor sigma quietly becoming an `R` matrix is the classic
way a simulator stops predicting the real vehicle.

`cars/<name>/sensors.toml` is the **registration**: which catalog models this car
carries, under what instance names, at what rate, and where they are mounted.
Extrinsics live here rather than in the catalog because a pose is a property of
the mounting, not of the part number. The convention is stated once, in the
`sensor` module docs: `v_parent = R * v_sensor + xyz`, metres, with `rpy` as
intrinsic Z-Y-X in radians, or a `[w, x, y, z]` quaternion instead for poses that
came out of a calibration tool.

```
cargo cev sensors            # the catalog
cargo cev sensors merceves   # what one car carries, resolved, with zenoh keys
```

Zenoh key expressions are derived as `cev/<car>/<sensor>`, so uniqueness within a
car is enough to guarantee it globally. Nothing publishes on them yet -- the
transport is the next piece of work, zenoh with its shared-memory backend so that
local subscribers are zero-copy and remote ones fall back to TCP through the same
API.

The catalog is group-level, so `cev deploy` rsyncs it to `<install_dir>/sensors`
in its own step -- it cannot ride inside `cars/<name>/`, and it cannot be merged
into `config/`, which is synced with `--delete`. Nodes find both directories
through `CEV_CAR_DIR` and `CEV_SENSORS_DIR`.

Still open: vehicle geometry proper -- wheelbase, track, wheel radius, CAN IDs.
It is not deployment truth and not sensor truth, and it still has no home. The
candidates are unchanged: a `hardware.toml` beside `car.toml` with its schema in
`cev-config`, or a repository of its own pulled in by gitman.

## What gitman pulls

| source | role | linked into `nodes/`? |
| --- | --- | --- |
| `rust-inekf` | state estimation | yes, `nodes/estimation` |
| `costmap-planner-rust` | planning | no -- ROS 2 / colcon, run as a `command` node |

Control, perception and logging have no repositories yet. When each one lands,
add it to `gitman.yml` following the pattern below.

Note that `gitman lock` and `gitman update` rewrite `gitman.yml` and delete every
comment except the header, so per-source commentary belongs in this table, not in
that file.

## Adding a node

1. Add a source to `gitman.yml`. If it is a plain Cargo package, give it
   `links: - target: nodes/<role>` and it joins the workspace build for free.
2. `uvx gitman install` then `uvx gitman lock`, and commit `gitman.yml`.
3. Add a `[[nodes]]` entry to the cars that should run it.
4. `cargo cev check`.

See [RUNNING.md](RUNNING.md) for setup and day-to-day commands.

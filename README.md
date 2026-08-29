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
├── cars/              one directory per vehicle -- the deployment manifests
│   ├── merceves/car.toml
│   └── sim/car.toml
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

Open question: per-car *hardware* constants -- wheelbase, track, wheel radius,
sensor extrinsics, CAN IDs -- are not deployment truth and are deliberately
absent from `car.toml`. They are needed by estimation, control and perception
alike, so they want one home rather than three copies. Nothing owns them yet;
the candidates are a `hardware.toml` beside `car.toml` with a schema in
`cev-config`, or a repository of their own pulled in by gitman.

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

# Design and status

## Why it works this way

**Why doesn't the stack pass data between nodes, or rename topics?**
Nodes talk to each other directly over Zenoh. Each contract writes its topic
names out in full, and the stack only checks them. With no middleman and no
renaming, the name in the contract is the name on the wire, so there is one
place to look when something does not connect.

**Why plain Cargo instead of a separate tool to fetch the node repos?**
The stack used to clone node repos with a separate tool (gitman) and link them
into one folder. Cargo already fetches git dependencies, records the exact
commit of each in `Cargo.lock`, and builds them, so one tool does it all.
Moving one node forward is `cargo update -p <node>`, and the `Cargo.lock` diff
*is* the version bump.

**Why does every node have to be a Cargo package?**
So every node is fetched, pinned and built the same way. A node in another
language can still join through a thin Rust wrapper package.

**Why TOML and not YAML?**
The stack already reads TOML for `car.toml`, and the common Rust YAML library
(`serde_yaml`) is no longer maintained.

**Why a separate `cev-node.toml` file?**
- **It is data, not code.** Anyone can read it without knowing the node's
  internals, and the stack can check it without compiling the node.
- **It lives with the node.** The person who changes what a node publishes
  updates the contract in the same commit.
- **It is its own file, not a `[package.metadata]` section of `Cargo.toml`**,
  so it reads on its own, and the same format could describe a non-Rust node
  later.

**Why must message types come from the `MsgType` list in `sensor-defs`?**
If both ends of a topic only had to write the same string, a typo on both
sides would pass. A fixed list means every type in a contract is one that
really exists in code.

**Why must a node list its modes?**
So it is off by default. If a new mode is added later, a node (a controller,
say) does not start in it until someone decides it should. Today modes only
affect `cargo cev check`; launch ignores them until launch by mode lands
(see [Status](#status), step 4).

**Why are nodes named by package, not by role?**
So two nodes with the same role can coexist, for example two estimators
(`holdover-ekf` and `rust-inekf`) while one replaces the other.

**Why does a GPU node need a `runtime` but not a `model`?**
Every GPU node needs some runtime (`cuda`, `tensorrt`, `onnx`, ...) on the
car, but some are plain parallel code with no model file.

**Why collect problems instead of stopping at the first?**
So one run of `cargo cev check` gives you most of the to-do list. It is not
complete yet: a broken `car.toml` or a contract with a field that is not
allowed still stops early; see [Checking](checking.md#what-it-does).

**Why are cars in this repo, not one repo per car?**
See [Cars live here](../../README.md#cars-live-here-not-in-a-per-car-repository)
in the root README.

**Why doesn't a bare `cargo build` build the nodes?**
`cev-nodes` is not a default workspace member. Some nodes (like `zed-reader`)
need SDKs a laptop does not have, so nodes are built only when asked for, with
`cargo build -p <node>` or by `cargo launch` / `cargo deploy`.

## Status

As of 2026-10-26, on the `node-contracts` branch.

**Done**

- The contract format and its per-contract rules, with tests.
- The switch to plain Cargo for fetching nodes (`crates/cev-nodes`, `Cargo.lock`).
- `holdover-ekf` ships a contract and both cars list it. Its program is still
  empty: it does not process data yet.
- `car.toml` has `kind` (sim or real), `modes` and `gpu`.
- `cargo cev check` loads every car node's contract and checks it against the
  car (shared mode, GPU), plus a test that does the same.
- A contract belongs to the package it ships in, so it does not name a
  package or a command. Its optional `[run]` (`bin`, `args`) is read but not
  used yet: `cargo launch` and `cargo deploy` still take the binary from the
  car's `[[nodes]]` entry.

**Next, in order (planned, not built)**

1. **Car entries name only a package.** `name`, `bin`, `command` and `args`
   leave `car.toml`; `cargo launch` and `cargo deploy` take how-to-run from
   the contract's `[run]`; add `cargo launch <car> --dry-run`.
2. **Real data flowing.** A wire format for `IMU`, `GPS` and `StateEstimate`;
   a simulated sensor node publishing `sensors/imu` at 200 Hz and
   `sensors/gps` at 20 Hz; and `holdover-ekf` subscribing and publishing over
   Zenoh.
3. **Graph check for each mode.** Every input has exactly one producer, of
   the same type, at a fast enough rate, in every mode the car supports.
4. **Launch by mode.** `cargo launch <car> --mode <mode>` starts only that
   mode's nodes, tells them the mode and which metrics to publish, and refuses
   to start if the graph check fails. Also: a Zenoh namespace per car, so two
   cars on one network cannot hear each other, and one standard setting for
   where every node finds its Zenoh config.
5. **Node-repo side.** A template contract, and a test in each node repo that
   loads its own contract. Maybe a `cargo cev graph` command that prints a
   diagram.

**Not in the stack yet:** `zed-reader` (camera; no contract until it is
ready), `rust-inekf` (pulled in, but no car runs it), and the costmap planner
(waiting on a move from ROS 2 to Zenoh).

## Still open

These have not been decided. Ask Lucas before building on any of them.

- What output rate `holdover-ekf` promises on `estimation/state`. The
  contract says 200 Hz (one estimate per IMU sample), but that is a proposal.
- What a `StateEstimate` contains, and how messages are encoded on the wire.
- Which values `runtime` may take. It is free text until the set is known.
- How `zed-reader` handles its "no depth" setting, which today drops its depth
  output without saying so.
- Where per-car hardware constants (wheelbase, sensor positions, CAN IDs)
  should live. They are deliberately not in `car.toml`.

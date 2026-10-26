# Checking

`cargo cev check` answers one question: can each car run the nodes it names?
Run it after any change to a car, a contract, or `Cargo.lock`.

```bash
cargo cev check            # every car
cargo cev check sim        # one car
```

`cargo test` runs the same check as a test, but the test stops at the first
problem, so use `cargo cev check` to see the full list.

## What it does

1. **Find the nodes.** It runs `cargo metadata`, a Cargo command that lists
   every package Cargo *resolved* (found and pinned a version of), including
   the node repos pulled in by `crates/cev-nodes`, and where each one is
   checked out on disk (under `~/.cargo/git`, or your local clone if you use
   a `[patch]`; see [RUNNING.md](../../RUNNING.md#working-on-a-node)).
2. **Load each car** in `cars/`. A car must have `kind`, `gpu` and a non-empty
   `modes` list.
3. **Load each node's contract** from `<package folder>/cev-node.toml` and
   apply the [contract rules](contracts.md#the-rules).
4. **Check the node against the car** (below).
5. **Report** what it found and exit with failure if anything was wrong.

How much it reports in one run:

- A broken `car.toml` stops the whole run at that car.
- A contract with a field that is not allowed reports only the first such
  field.
- A contract that breaks several rules lists them all, but counts as one in
  the final `N problem(s)` line.
- Otherwise every problem on every car is printed.

## Node against car

These checks need both the contract and the car. The car's side, from
`cars/merceves/car.toml`:

```toml
name = "merceves"
description = "MerCEVes autonomous testbed (Jetson)"
kind = "real"
modes = ["teleop", "autonomous"]
gpu = true
```

| Rule | You'll see | Fails when |
| --- | --- | --- |
| The node and the car share at least one mode | `shares no mode with the car, so it would never start` | No mode in the contract's `modes` is in the car's `modes` |
| A GPU node is on a car with a GPU | ``needs a GPU (runtime `tensorrt`), but the car has `gpu = false` `` | The contract says `device = "gpu"` and the car says `gpu = false` |

The car lists the modes it supports; the node lists the modes it runs in.
Today `check` only tests that the two lists overlap. Starting only the nodes
for one mode (`cargo launch <car> --mode <mode>`) is planned, not built.

## What success looks like

Real output on the current branch:

```text
 CHECKING  car merceves (1 node(s))
 FOUND     holdover-ekf /home/you/.cargo/git/checkouts/holdover-ekf-9d61a9254c61a930/d62d50e/cev-node.toml
 CHECKING  car sim (1 node(s))
 FOUND     holdover-ekf /home/you/.cargo/git/checkouts/holdover-ekf-9d61a9254c61a930/d62d50e/cev-node.toml
 FINISHED  no problems
```

The last folder in the path is the commit `Cargo.lock` pins, so yours may
differ.

## What failure looks like

Say someone writes a contract for a new camera detector, `my-detector`. It
needs a GPU and runs only in `efficiency` mode (driving for low energy use,
which neither car lists yet). It also has three mistakes: a leading `/` on a
topic, a message type that does not exist, and a rate of zero:

```toml
contract_version = 1
role = "perception"
modes = ["efficiency"]

[compute]
device = "gpu"
runtime = "tensorrt"

[[inputs]]
topic = "/sensors/zed/image"
type = "ZedImage"
min_rate_hz = 15.0

[[outputs]]
topic = "perception/obstacles"
type = "Obstacles"
rate_hz = 0.0
```

They add it to the `sim` car, which has:

```toml
modes = ["teleop", "autonomous"]
gpu = false
```

(`gpu = false` on `sim` is deliberate: the sim car has to run on a Mac too.)
The output of `cargo cev check sim` below is real, with long paths shortened
to `...`.

**Rule violations** are listed together:

```text
 CHECKING  car sim (2 node(s))
 FOUND     holdover-ekf .../cev-node.toml
 ERROR     .../my-detector/cev-node.toml: contract violations:
 - topic `/sensors/zed/image` is not a usable Zenoh key: no leading or trailing `/`
 - topic `perception/obstacles`: unknown type `Obstacles` (not a sensor_defs::MsgType)
 - topic `perception/obstacles`: 0 Hz is not a positive, finite rate
 ERROR     1 problem(s)
```

**Node against car.** Fix those three, and the contract passes on its own,
but it does not fit this car:

```text
 CHECKING  car sim (2 node(s))
 FOUND     holdover-ekf .../cev-node.toml
 FOUND     my-detector .../my-detector/cev-node.toml
 ERROR     my-detector: shares no mode with the car, so it would never start
 ERROR     my-detector: needs a GPU (runtime `tensorrt`), but the car has `gpu = false`
 ERROR     2 problem(s)
```

**A field that is not allowed.** Write `min_rate = 15.0` instead of
`min_rate_hz = 15.0`, and the file is refused before any rule is checked:

```text
 ERROR     parsing .../my-detector/cev-node.toml: TOML parse error at line 12, column 1
   |
12 | min_rate = 15.0
   | ^^^^^^^^
unknown field `min_rate`, expected one of `topic`, `type`, `min_rate_hz`
```

**A package the stack does not pull in.** If a car names a package that is not
a dependency in `crates/cev-nodes/Cargo.toml`:

```text
 ERROR     node package `my-detector` is not resolved (is it a dependency in crates/cev-nodes/Cargo.toml?)
```

## Not checked yet

The biggest missing piece is checking nodes **against each other**. It is
planned (the "graph check"), not built. For every mode a car supports, it will
check that:

- every input topic has exactly one node publishing it,
- the publisher's type matches the subscriber's type,
- the publisher's `rate_hz` is at least the subscriber's `min_rate_hz`.

For example, today both cars list only `holdover-ekf` (which does not process
data yet), and nothing in the stack publishes `sensors/imu` or `sensors/gps`.
The current check passes; the graph check will flag it. A simulated sensor
node that publishes those topics is planned.

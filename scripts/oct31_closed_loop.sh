#!/bin/sh
# Starts the oct-31 bridge, the sim stack, and the goal publisher.
# The orchestrator, the sim dev server, and the oct-31-testing page must already be up.
# Start the episode in the browser once the stack is running.
#
# Pass: global grid about 1 Hz and lined up with the map, A* trajectories,
# stop within 1 m of the goal 10 m ahead, estimate close to cev/sim/truth/pose.
# If the Zenoh peers never meet, start zenohd on this machine and rerun.
set -eu
ROOT=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
ORCH=${OCT31_ORCH:-$HOME/Coding/py/orchestrator}
cd "$ROOT"

listening() {
    python3 -c 'import socket,sys; socket.create_connection((sys.argv[1], int(sys.argv[2])), 2).close()' 127.0.0.1 "$1"
}

listening 8080 || {
    echo "Nothing is listening on 8080. Start the orchestrator: (cd \"$ORCH\" && python3 main.py)" >&2
    exit 1
}
listening 8090 || {
    echo "Nothing is listening on 8090. The orchestrator types API is not up." >&2
    exit 1
}
listening 3000 || {
    echo "Nothing is listening on 3000. Start the simulator dev server, open oct-31-testing, then rerun." >&2
    exit 1
}

bridge_log=$(mktemp)
goal_log=$(mktemp)
bridge_pid=""
goal_pid=""
cleanup() {
    if [ -n "$bridge_pid" ]; then kill "$bridge_pid" 2>/dev/null || true; fi
    if [ -n "$goal_pid" ]; then kill "$goal_pid" 2>/dev/null || true; fi
    rm -f "$bridge_log" "$goal_log"
}
trap cleanup EXIT

(cd "$ORCH" && python3 client/oct31_zenoh_bridge.py) >"$bridge_log" 2>&1 &
bridge_pid=$!
i=0
while [ "$i" -lt 50 ]; do
    if grep -q "Transport ready" "$bridge_log"; then
        break
    fi
    if ! kill -0 "$bridge_pid" 2>/dev/null; then
        cat "$bridge_log" >&2
        exit 1
    fi
    i=$((i + 1))
    sleep 0.2
done
if ! grep -q "Transport ready" "$bridge_log"; then
    echo "Bridge did not become ready. Is the oct-31-testing page connected?" >&2
    cat "$bridge_log" >&2
    exit 1
fi
echo "Bridge ready. Starting the stack. Start the episode in the browser."

python3 scripts/fake_goal.py >"$goal_log" 2>&1 &
goal_pid=$!
sleep 0.5
if ! kill -0 "$goal_pid" 2>/dev/null; then
    cat "$goal_log" >&2
    exit 1
fi
set +e
cargo launch sim
status=$?
set -e
echo "Goal publisher log:"
cat "$goal_log"
exit "$status"

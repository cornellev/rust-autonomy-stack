"""Publish a goal straight ahead of the first pose, and a constant speed.

The goal key is meters in disguise: longitude is x, latitude is y. It is
re-sent at 1 Hz because Zenoh does not keep the last sample for a late
planner. Speed is m/s on cev/config/speed. At the arrival radius the speed
goes to zero and stays there.

    python scripts/fake_goal.py
    python scripts/fake_goal.py --distance 10 --speed 0.8 --arrive-radius 1
"""

from __future__ import annotations

import argparse
import json
import math
import sys
import threading
import time


POSE_KEY = "cev/localization/pose"
GOAL_KEY = "cev/commands/goal"
SPEED_KEY = "cev/config/speed"


def goal_ahead(x, y, heading, distance):
    return x + distance * math.cos(heading), y + distance * math.sin(heading)


def _check():
    gx, gy = goal_ahead(0.0, 0.0, 0.0, 10.0)
    if abs(gx - 10.0) > 1e-9 or abs(gy) > 1e-9:
        raise SystemExit(f"goal ahead of +x failed: {gx}, {gy}")
    gx, gy = goal_ahead(1.0, 2.0, math.pi / 2, 4.0)
    if abs(gx - 1.0) > 1e-9 or abs(gy - 6.0) > 1e-9:
        raise SystemExit(f"goal ahead of +y failed: {gx}, {gy}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--distance", type=float, default=10.0)
    parser.add_argument("--speed", type=float, default=0.8)
    parser.add_argument("--arrive-radius", type=float, default=1.0)
    parser.add_argument("--check", action="store_true", help="run the goal math check and exit")
    args = parser.parse_args()
    if args.distance <= 0 or args.arrive_radius <= 0 or not math.isfinite(args.speed):
        raise SystemExit("distance and arrive-radius must be positive, and speed must be finite")
    if args.check:
        _check()
        return 0

    try:
        import zenoh
    except ImportError as error:
        raise SystemExit("Install eclipse-zenoh==1.10.1") from error

    pose = {"value": None}
    lock = threading.Lock()

    def on_pose(sample):
        try:
            body = json.loads(sample.payload.to_bytes())
            x = float(body["location"]["x"])
            y = float(body["location"]["y"])
            heading = float(body["heading"])
            if not (math.isfinite(x) and math.isfinite(y) and math.isfinite(heading)):
                raise ValueError("non-finite pose")
        except (ValueError, TypeError, KeyError, UnicodeError) as error:
            print(f"ignored pose: {error}", file=sys.stderr)
            return
        with lock:
            pose["value"] = (x, y, heading)

    with zenoh.open(zenoh.Config()) as session:
        session.declare_subscriber(POSE_KEY, on_pose)
        goal_pub = session.declare_publisher(GOAL_KEY)
        speed_pub = session.declare_publisher(SPEED_KEY)
        goal = None
        arrived = False
        print(f"waiting for {POSE_KEY}", flush=True)
        while True:
            with lock:
                current = pose["value"]
            if goal is None:
                if current is None:
                    time.sleep(0.05)
                    continue
                goal = goal_ahead(*current[:2], current[2], args.distance)
                print(f"goal {goal[0]:.3f}, {goal[1]:.3f}", flush=True)
            x, y, _heading = current
            if not arrived and math.hypot(goal[0] - x, goal[1] - y) <= args.arrive_radius:
                arrived = True
                print("arrived, speed 0", flush=True)
            speed_pub.put(json.dumps({"raw": 0.0 if arrived else args.speed}))
            goal_pub.put(json.dumps({"longitude": goal[0], "latitude": goal[1]}))
            time.sleep(1.0)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except KeyboardInterrupt:
        raise SystemExit(0)

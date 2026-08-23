#!/usr/bin/env python3
"""Emit curve-setpoint vectors from Breeze Core's *own* scheduler.

Run against a checkout of the Python reference, with a virtualenv that has its
dependencies, so the imports below resolve to the real implementation:

    PYTHONPATH=/path/to/breeze-core venv/bin/python \
        generate-curve-vectors.py > curve-vectors.json

The committed vectors were generated from a tree whose `programs/scheduler.py`
and `programs/models.py` hash identically to breeze-core 3.2.0 -- worth checking
before regenerating, because more than one copy of this project exists on a
typical machine and the stale ones differ.

The point is that nothing here re-implements the maths. The vectors record what
the reference actually returns, so the Rust port is checked against it rather
than against my reading of it. That has already caught a real disagreement:
Python's round() is banker's rounding and Rust's f64::round is not, which shifts
every exact midpoint by half a degree.

Sampling every 7th minute deliberately avoids landing on the hour, where a
naive implementation happens to be right.
"""
import json
import random
from datetime import datetime

from meow_ac.programs.models import CurvePoint
from meow_ac.programs.scheduler import _round_half, curve_setpoint

random.seed(20260823)
out = {"round_half": [], "curve": []}

# Every quarter degree across the range (so every exact .5 midpoint of the
# doubled value is hit), some eighths, and the clamp edges.
vals = [x / 4 for x in range(4 * 10, 4 * 36)]
vals += [-40.0, 0.0, 15.9, 16.0, 30.0, 30.1, 99.0]
vals += [x / 8 for x in range(8 * 19, 8 * 24)]
for v in vals:
    out["round_half"].append([v, _round_half(v)])

cases = [
    [("08:00", 22.0)],                                        # single point
    [("00:00", 20.0), ("12:00", 26.0)],                       # plain ramp
    [("08:00", 24.0), ("22:00", 20.0)],                       # wraps midnight
    [("22:00", 20.0), ("08:00", 24.0)],                       # same, unsorted
    [("00:00", 16.0), ("06:00", 30.0), ("18:00", 16.0)],      # full range
    [("07:30", 21.0), ("09:45", 23.5), ("13:15", 25.0), ("23:50", 18.0)],
    [("00:00", 20.0), ("00:00", 25.0)],                       # zero-length span
    [("00:00", 21.3), ("23:59", 24.7)],                       # 1-minute wrap
    [("01:00", 19.25), ("02:00", 19.75)],                     # midpoint bait
]
# Plus random shapes, because hand-picked cases test what I thought of.
for _ in range(12):
    n = random.randint(2, 6)
    times = sorted(random.sample(range(1440), n))
    cases.append(
        [
            ("%02d:%02d" % (t // 60, t % 60), round(random.uniform(16, 30), 2))
            for t in times
        ]
    )

for pairs in cases:
    pts = [CurvePoint(time=t, temperature=c) for t, c in pairs]
    samples = []
    for mins in range(0, 1440, 7):
        now = datetime(2026, 8, 23, mins // 60, mins % 60, 13)
        samples.append([mins, curve_setpoint(pts, now)])
    out["curve"].append(
        {
            "points": [{"time": t, "temperature": c} for t, c in pairs],
            "samples": samples,
        }
    )

print(json.dumps(out))

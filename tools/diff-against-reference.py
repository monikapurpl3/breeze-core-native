#!/usr/bin/env python3
"""Compare the two servers' responses byte for byte, not just semantically.

Native on 8421, the Python 3.2.0 reference on 8422. Key order counts here: the
point of the exercise is that a client cannot tell them apart, and comparing
parsed objects would hide exactly the class of difference this is checking for.

Empty responses fail rather than match, and the run fails if fewer than a
sensible number of comparisons actually happened.
"""
import json
import subprocess
import sys
import urllib.error
import urllib.request

KEY = open("/tmp/bcn.key").read().strip()
TOKENS = {
    "native": open("/tmp/bcn.token").read().strip(),
    "python": open("/tmp/bcn-py.token").read().strip(),
}
PORTS = {"native": 8421, "python": 8422}

checked = 0
identical = 0
differences = []


def call(impl, method, path, body=None):
    url = f"http://127.0.0.1:{PORTS[impl]}{path}"
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, method=method)
    req.add_header("X-API-Key", KEY)
    req.add_header("Authorization", f"Bearer {TOKENS[impl]}")
    req.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(req, timeout=120) as r:
            return r.status, r.read().decode()
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode()
    except Exception as e:
        return None, f"<transport error: {e}>"


def blank_volatile(text):
    """Remove what legitimately differs between two live servers."""
    try:
        v = json.loads(text)
    except Exception:
        return text

    def walk(x):
        if isinstance(x, dict):
            out = {}
            for k, val in x.items():
                if k in (
                    # Live readings drift between two calls seconds apart.
                    "indoor_temperature", "outdoor_temperature",
                    # Server-assigned or per-process.
                    "id", "token_id", "created_at", "last_used", "expires_at",
                    "last_run", "runs", "uptime_seconds", "started_at",
                    # Version and build identity are meant to differ.
                    "version", "build", "commit", "features", "python",
                    "auth_versions", "min_auth_version",
                ):
                    out[k] = "<volatile>"
                else:
                    out[k] = walk(val)
            return out
        if isinstance(x, list):
            return [walk(i) for i in x]
        return x

    # Re-serialise preserving order, so key order still participates.
    return json.dumps(walk(v), separators=(",", ":"))


def check(label, method, path, body=None):
    global checked, identical
    sn, bn = call("native", method, path, body)
    sp, bp = call("python", method, path, body)
    checked += 1

    if sn is None or sp is None:
        differences.append(f"{label}: transport failure\n    native: {bn}\n    python: {bp}")
        print(f"  ERROR   {label}")
        return
    if not bn.strip() and sn != 204:
        differences.append(f"{label}: native returned an empty body at {sn}")
        print(f"  EMPTY   {label}")
        return

    cn, cp = blank_volatile(bn), blank_volatile(bp)
    if sn == sp and cn == cp:
        identical += 1
        print(f"  same    [{sn}] {label}")
    else:
        differences.append(
            f"{label}\n    native [{sn}]: {cn[:400]}\n    python [{sp}]: {cp[:400]}"
        )
        print(f"  DIFFER  {label}")


UNIT = "153931628470980"

print("=== read paths ===")
check("health", "GET", "/api/health")
check("units", "GET", "/api/units")
check("one unit's state", "GET", f"/api/units/{UNIT}/state")
check("batch state", "GET", "/api/units/state")
check("config", "GET", "/api/config")
check("whoami", "GET", "/api/auth/whoami")

print("\n=== programs ===")
check("programs (empty)", "GET", "/api/programs")
check("scheduler status", "GET", "/api/programs/status")
check(
    "create a favourite",
    "POST",
    "/api/programs",
    {
        "name": "Diff",
        "kind": "favourite",
        "favourite": {
            "power_state": True,
            "operational_mode": "COOL",
            "target_temperature": 23.5,
            "fan_speed": 102,
            "swing_mode": "BOTH",
            "eco": True,
            "turbo": False,
        },
    },
)
check(
    "create a curve",
    "POST",
    "/api/programs",
    {
        "name": "Diff curve",
        "kind": "curve",
        "enabled": False,
        "curve": {
            "operational_mode": "heat",
            "fan_speed": 60,
            "points": [
                {"time": "00:00", "temperature": 20.0},
                {"time": "12:00", "temperature": 23.5},
            ],
        },
    },
)
check("programs (populated)", "GET", "/api/programs")

print("\n=== timers ===")
check("timers (empty)", "GET", "/api/timers")
check("timer status", "GET", "/api/timers/status")
check(
    "create a timer",
    "POST",
    "/api/timers",
    {"unit_ids": [UNIT], "minutes": 45, "label": "diff"},
)
check("timers (populated)", "GET", "/api/timers")

print("\n=== errors ===")
check("unknown unit", "GET", "/api/units/999/state")
check("unknown program", "GET", "/api/programs/nope")
check("bad control", "POST", f"/api/units/{UNIT}/control", {"target_temperature": 99})
check("unknown path", "GET", "/api/nothing-here")

print("\n=== the panel, byte for byte ===")
for asset in ("/", "/index.html", "/js/app.js", "/js/api.js", "/css/styles.css"):
    a = subprocess.run(
        ["curl", "-s", f"http://127.0.0.1:8421{asset}"], capture_output=True
    ).stdout
    b = subprocess.run(
        ["curl", "-s", f"http://127.0.0.1:8422{asset}"], capture_output=True
    ).stdout
    checked += 1
    if a == b and a:
        identical += 1
        print(f"  same    [{len(a)} bytes] {asset}")
    else:
        differences.append(f"panel {asset}: native {len(a)} bytes, python {len(b)} bytes")
        print(f"  DIFFER  {asset}")

print("\n" + "=" * 62)
print(f"compared {checked}, identical {identical}, differing {len(differences)}")
if checked < 25:
    print("TOO FEW COMPARISONS -- something did not run")
    sys.exit(1)
if differences:
    print("\nDifferences:")
    for d in differences:
        print(f"\n- {d}")
sys.exit(1 if differences else 0)

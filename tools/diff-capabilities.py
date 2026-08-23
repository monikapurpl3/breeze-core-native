#!/usr/bin/env python3
"""Compare `/api/units/{id}/capabilities` between the two servers, per unit.

The capability reply is the one part of the protocol where a single byte means
"which set of features you get" rather than a boolean, decoded through tables
that exist only in msmart's source. So this is the check that matters: the two
servers ask the same hardware and must derive the same answer.

Native on 8421, the Python reference on 8422; see `tools/README.md` for how to
stand a pair up. Needs an API key in /tmp/bcn.key and a device token per server
in /tmp/bcn.token and /tmp/bcn-py.token.
"""
import json
import sys
import urllib.error
import urllib.request

KEY = open("/tmp/bcn.key").read().strip()
TOKENS = {
    8421: open("/tmp/bcn.token").read().strip(),
    8422: open("/tmp/bcn-py.token").read().strip(),
}


def get(port, path):
    req = urllib.request.Request(f"http://127.0.0.1:{port}{path}")
    req.add_header("X-API-Key", KEY)
    req.add_header("Authorization", f"Bearer {TOKENS[port]}")
    try:
        with urllib.request.urlopen(req, timeout=180) as r:
            return r.status, json.loads(r.read())
    except urllib.error.HTTPError as e:
        return e.code, {"detail": e.read().decode()[:200]}
    except Exception as e:
        return None, {"detail": f"transport: {e}"}


status, config = get(8421, "/api/config")
if status != 200:
    print(f"cannot read the unit list: {status} {config}")
    sys.exit(1)
units = [u["id"] for u in config.get("units", [])]
print(f"comparing {len(units)} unit(s)\n")

problems = []
for unit in units:
    print(f"=== unit {unit} ===")
    sn, native = get(8421, f"/api/units/{unit}/capabilities")
    sp, python = get(8422, f"/api/units/{unit}/capabilities")

    if sn != 200 or sp != 200:
        print(f"  native [{sn}] {native}")
        print(f"  python [{sp}] {python}")
        problems.append(f"unit {unit}: native {sn}, python {sp}")
        continue

    # Compare only fields the reference actually reports; anything extra this
    # server adds is additive and cannot break a client.
    shared = [k for k in python if k in native]
    missing = [k for k in python if k not in native]
    extra = [k for k in native if k not in python]
    diffs = [(k, native[k], python[k]) for k in shared if native[k] != python[k]]

    print(f"  reference fields: {len(python)}, compared: {len(shared)}")
    if missing:
        print(f"  MISSING from native: {missing}")
        problems.append(f"unit {unit}: missing {missing}")
    if extra:
        print(f"  extra (additive, fine): {extra}")
    if diffs:
        for key, a, b in diffs:
            print(f"  DIFFER {key}: native={a!r} python={b!r}")
            problems.append(f"unit {unit}: {key} native={a!r} python={b!r}")
    else:
        print("  every shared field identical")

    print(f"  modes:  {native.get('operational_modes')}")
    print(f"  swing:  {native.get('swing_modes')}")
    print(f"  fan:    {native.get('fan_speeds')}")
    print(
        f"  temps:  {native.get('min_target_temperature')}"
        f"-{native.get('max_target_temperature')}"
    )
    unrecognised = native.get("unrecognised_capability_ids")
    if unrecognised:
        print(f"  capability ids not interpreted: {unrecognised}")
    print()

print("=" * 60)
if problems:
    print(f"{len(problems)} problem(s):")
    for p in problems:
        print(f"  - {p}")
    sys.exit(1)
print("capabilities agree on every unit")

#!/usr/bin/env python3
"""Compare a running Breeze Core 4.x against a running 3.x, endpoint by endpoint.

    python3 differential.py --rust http://127.0.0.1:18420 \
                            --python http://127.0.0.1:8420 \
                            --config /etc/breeze-core/config.json

Both servers must be pointed at the SAME config.json, so the only thing that
differs is the implementation. Every request here is a read or a deliberately
invalid write that is refused by validation before any LAN traffic happens, so
this never touches an air conditioner.

Why this exists: 4.0.0's contract with its clients is "byte-for-byte what 3.2.0
sent". Unit tests check the implementation against its own idea of that
contract, which is exactly the check that cannot catch a misunderstanding. The
reference is still installable, so the honest test is to ask both and diff.

It reports three kinds of finding, and the middle one is the dangerous one:

  status   different HTTP status for the same request
  type     same value, different JSON type -- 30 vs 30.0, "1" vs 1
  shape    a key one has and the other does not

A type difference is invisible to a JavaScript client and fatal to a strictly
typed one: Dart's `as int` throws on 30.0, so a float where the reference sent
an integer breaks the Android app while the web panel shrugs.
"""
import argparse
import json
import sys
import urllib.error
import urllib.request

# Fields whose values legitimately differ between two processes, or between two
# moments. Compared for TYPE and presence, never for equality.
# Compared as sets: order is not part of the contract.
SET_LIKE = {"features", "operational_modes", "fan_speeds", "swing_modes"}
# Genuinely implementation-specific, and not a contract: the panel's diagnostics
# list what the server is built from, so 3.x names Python packages (fastapi,
# anyio, httpx) and 4.x names Rust crates. Comparing them produces dozens of
# findings that are all correct and none actionable.
IMPLEMENTATION_SPECIFIC = {"components"}
# Length depends on process uptime, not on correctness.
COUNT_VARIES = {"samples", "history", "devices"}

VOLATILE = {
    "version", "commit", "uptime_seconds", "started_at", "now", "server_time",
    "indoor_temperature", "outdoor_temperature", "indoor_humidity",
    "last_used", "created_at", "expires_at", "seconds_remaining", "fires_at",
    "latency_ms", "samples", "history", "token_id", "session_id", "user_code",
    "expires_in", "next_run", "last_run", "pid", "python", "rust", "platform",
    "implementation", "generated_at", "timestamp",
    # A history sample's own timestamp: two processes that started at different
    # moments have sampled at different moments, always.
    "t",
}


def get(base, path, key, token=None, method="GET", body=None):
    headers = {"X-API-Key": key}
    if token:
        headers["Authorization"] = "Bearer " + token
    data = None
    if body is not None:
        data = json.dumps(body).encode()
        headers["Content-Type"] = "application/json"
    req = urllib.request.Request(base + path, data=data, headers=headers, method=method)
    try:
        with urllib.request.urlopen(req, timeout=30) as r:
            raw = r.read()
            return r.status, raw
    except urllib.error.HTTPError as e:
        return e.code, e.read()
    except Exception as e:  # a connection failure is itself a finding
        return None, str(e).encode()


def enrol(base, key, label):
    """A v1 bearer credential, so the comparison is not about signing."""
    s, raw = get(base, "/api/auth/enroll/start", key, method="POST",
                 body={"label": label, "auth_version": 1})
    if s != 200:
        return None
    start = json.loads(raw)
    get(base, "/api/auth/enroll/approve", key, method="POST",
        body={"code": start["user_code"]})
    s, raw = get(base, "/api/auth/enroll/poll", key, method="POST",
                 body={"session_id": start["session_id"]})
    return json.loads(raw).get("device_token") if s == 200 else None


def jtype(v):
    if isinstance(v, bool):
        return "bool"
    if isinstance(v, int):
        return "int"
    if isinstance(v, float):
        return "float"
    if v is None:
        return "null"
    return type(v).__name__


def compare(path, a, b, findings, where=""):
    """Structural diff. `a` is Rust, `b` is the Python reference."""
    if isinstance(a, dict) and isinstance(b, dict):
        for k in sorted(set(a) | set(b)):
            at = f"{where}.{k}" if where else k
            if k in IMPLEMENTATION_SPECIFIC:
                continue
            if k not in a:
                findings.append(("shape", path, at, "missing in 4.x", jtype(b[k])))
            elif k not in b:
                findings.append(("shape", path, at, "extra in 4.x", jtype(a[k])))
            else:
                compare(path, a[k], b[k], findings, at)
        return
    if isinstance(a, list) and isinstance(b, list):
        leaf_name = where.split(".")[-1].split("[")[0]
        # Sets, not sequences: `features` is a capability list a client tests
        # membership in, and 4.x sorts it while the reference does not. Ordering
        # there is not part of the contract, and comparing it as a sequence
        # buries every real finding under fifteen false ones.
        if leaf_name in SET_LIKE:
            only_4x, only_3x = set(a) - set(b), set(b) - set(a)
            if only_4x:
                findings.append(("shape", path, where, f"extra: {sorted(only_4x)}", "-"))
            if only_3x:
                findings.append(("shape", path, where, "-", f"missing: {sorted(only_3x)}"))
            return
        # Sample COUNTS differ purely because the two processes started at
        # different times; the shape of each sample is what matters.
        if leaf_name in COUNT_VARIES:
            for i, (x, y) in enumerate(zip(a, b)):
                compare(path, x, y, findings, f"{where}[{i}]")
            return
        if len(a) != len(b):
            findings.append(("shape", path, where or "<root>",
                             f"length {len(a)}", f"length {len(b)}"))
        for i, (x, y) in enumerate(zip(a, b)):
            compare(path, x, y, findings, f"{where}[{i}]")
        return
    ta, tb = jtype(a), jtype(b)
    # int vs float is THE interesting one: JSON has one number type, consumers
    # do not, and 30.0 where the reference sent 30 breaks a typed client.
    if ta != tb:
        findings.append(("type", path, where or "<root>", f"{ta} {a!r}", f"{tb} {b!r}"))
        return
    leaf = where.split(".")[-1].split("[")[0]
    if leaf in VOLATILE:
        return
    if a != b:
        findings.append(("value", path, where or "<root>", repr(a), repr(b)))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--rust", required=True)
    ap.add_argument("--python", required=True)
    ap.add_argument("--config", default="/etc/breeze-core/config.json")
    args = ap.parse_args()

    cfg = json.load(open(args.config))
    key = cfg["api_key"]
    units = [str(u["id"]) for u in cfg.get("units", [])]

    tr = enrol(args.rust, key, "differential")
    tp = enrol(args.python, key, "differential")
    if not tr or not tp:
        print("could not enrol against both servers", file=sys.stderr)
        return 2

    cases = [
        ("GET", "/api/health", None),
        ("GET", "/api/version", None),
        ("GET", "/api/units", None),
        ("GET", "/api/units/state", None),
        ("GET", "/api/config", None),
        ("GET", "/api/system", None),
        ("GET", "/api/programs", None),
        ("GET", "/api/programs/status", None),
        ("GET", "/api/timers", None),
        ("GET", "/api/timers/status", None),
        ("GET", "/api/auth/devices", None),
        ("GET", "/api/auth/whoami", None),
        # Errors are part of the contract too -- clients branch on them.
        ("GET", "/api/units/99999999999/state", None),
        ("GET", "/api/units/not-a-number/state", None),
        ("GET", "/api/nope", None),
        ("POST", "/api/health", None),
        # Refused by validation before any LAN traffic, so no unit is touched.
        ("POST", f"/api/units/{units[0]}/control", {"target_temperature": 99}),
        ("POST", f"/api/units/{units[0]}/control", {"operational_mode": "TELEPORT"}),
        ("POST", f"/api/units/{units[0]}/control", {"fan_speed": 7}),
        ("POST", f"/api/units/{units[0]}/control", {"swing_mode": "DIAGONAL"}),
    ]
    for u in units:
        cases += [
            ("GET", f"/api/units/{u}/state", None),
            ("GET", f"/api/units/{u}/capabilities", None),
            ("GET", f"/api/units/{u}/history", None),
        ]

    findings = []
    checked = 0
    for method, path, body in cases:
        sa, ra = get(args.rust, path, key, tr, method, body)
        sb, rb = get(args.python, path, key, tp, method, body)
        checked += 1
        label = f"{method} {path}" + (f" {json.dumps(body)}" if body else "")
        if sa != sb:
            findings.append(("status", label, "<http>", str(sa), str(sb)))
            continue
        try:
            ja, jb = json.loads(ra or b"null"), json.loads(rb or b"null")
        except json.JSONDecodeError:
            if ra != rb:
                findings.append(("body", label, "<non-json>",
                                 (ra or b"")[:60].decode("utf8", "replace"),
                                 (rb or b"")[:60].decode("utf8", "replace")))
            continue
        compare(label, ja, jb, findings)

    print(f"compared {checked} requests against the reference\n")
    if not findings:
        print("no differences")
        return 0
    kinds = {}
    for f in findings:
        kinds[f[0]] = kinds.get(f[0], 0) + 1
    print("  " + ", ".join(f"{v} {k}" for k, v in sorted(kinds.items())) + "\n")
    for kind, path, where, a, b in findings:
        print(f"  [{kind}] {path}")
        print(f"        at {where}")
        print(f"        4.x: {a}")
        print(f"        3.x: {b}")
    return 1


if __name__ == "__main__":
    sys.exit(main())

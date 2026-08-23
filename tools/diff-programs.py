#!/usr/bin/env python3
"""Send identical /api/programs requests to both servers and diff the answers.

Native on 8421, the Python 3.2.0 reference on 8422, each with its own throwaway
store. Server-assigned ids are blanked before comparing; everything else must be
identical, status code included.

An empty response counts as a failure, not a match -- an earlier shell version of
this harness reported seventeen matches while curl was not even on the PATH.
"""
import json
import sys
import urllib.error
import urllib.request

KEY = open("/tmp/bcn.key").read().strip()
TOKENS = {
    "native": open("/tmp/bcn.token").read().strip(),
    "python": open("/tmp/bcn-py.token").read().strip(),
}
PORTS = {"native": 8421, "python": 8422}

results = {"match": 0, "differ": 0, "error": 0}
failures = []


def call(impl, method, path, body=None):
    url = f"http://127.0.0.1:{PORTS[impl]}{path}"
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, method=method)
    req.add_header("X-API-Key", KEY)
    req.add_header("Authorization", f"Bearer {TOKENS[impl]}")
    req.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(req, timeout=30) as r:
            return r.status, r.read().decode()
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode()
    except Exception as e:                      # connection refused, timeout
        return None, f"<transport error: {e}>"


def scrub(x):
    """Blank the fields that are allowed to differ."""
    if isinstance(x, dict):
        return {k: ("<id>" if k == "id" else scrub(v)) for k, v in x.items()}
    if isinstance(x, list):
        return [scrub(i) for i in x]
    return x


def canonical(text):
    try:
        return json.dumps(scrub(json.loads(text)), sort_keys=False)
    except Exception:
        return f"<not json: {text[:200]}>"


def check(label, method, path, body=None, expect_status=None):
    sn, bn = call("native", method, path, body)
    sp, bp = call("python", method, path, body)

    if sn is None or sp is None:
        results["error"] += 1
        failures.append(f"{label}: transport failure\n    native: {bn}\n    python: {bp}")
        print(f"  ERROR  {label}")
        return None, None

    cn, cp = canonical(bn), canonical(bp)
    same = sn == sp and cn == cp
    if same and expect_status is not None and sn != expect_status:
        same = False
        failures.append(f"{label}: both agreed on {sn}, expected {expect_status}")

    if same:
        results["match"] += 1
        print(f"  match  [{sn}] {label}")
    else:
        results["differ"] += 1
        failures.append(
            f"{label}\n    native [{sn}]: {cn}\n    python [{sp}]: {cp}"
        )
        print(f"  DIFFER {label}")
    return bn, bp


def wipe():
    """Empty both stores so each run starts from the same place."""
    for impl in PORTS:
        _, body = call(impl, "GET", "/api/programs")
        try:
            for p in json.loads(body):
                call(impl, "DELETE", f"/api/programs/{p['id']}")
        except Exception:
            pass


FAVOURITE = {
    "name": "Ljeto",
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
}
SCHEDULE = {
    "name": "Weekday",
    "kind": "schedule",
    "schedule": [
        {
            "days": [4, 0, 4, 2],           # unsorted, duplicated on purpose
            "time": "07:30",
            "settings": {"power_state": True, "operational_mode": "COOL"},
        }
    ],
}
CURVE = {
    "name": "Noc",
    "kind": "curve",
    "enabled": False,
    "curve": {
        "operational_mode": "heat",         # lower case on purpose
        "fan_speed": 60,
        "points": [
            {"time": "00:00", "temperature": 20.0},
            {"time": "12:00", "temperature": 23.5},
        ],
    },
}

print("=== empty store, and things that do not exist ===")
wipe()
check("list empty", "GET", "/api/programs", expect_status=200)
check("get unknown", "GET", "/api/programs/nope", expect_status=404)
check("put unknown", "PUT", "/api/programs/nope", FAVOURITE, expect_status=404)
check("delete unknown", "DELETE", "/api/programs/nope", expect_status=404)
check("apply unknown", "POST", "/api/programs/nope/apply", expect_status=404)

print("\n=== rejected input ===")
check("nameless", "POST", "/api/programs", {"name": ""}, expect_status=422)
check("no name key", "POST", "/api/programs", {"kind": "curve"}, expect_status=422)
check("bad kind", "POST", "/api/programs", {"name": "x", "kind": "cronjob"}, expect_status=422)
check("65-char name", "POST", "/api/programs", {"name": "a" * 65}, expect_status=422)
check(
    "day 7",
    "POST",
    "/api/programs",
    {"name": "x", "kind": "schedule",
     "schedule": [{"days": [7], "time": "07:30", "settings": {}}]},
    expect_status=422,
)
for bad_time in ("7:30", "24:00", "07:60", "0730", "07:3"):
    check(
        f"time {bad_time!r}",
        "POST",
        "/api/programs",
        {"name": "x", "kind": "schedule",
         "schedule": [{"days": [], "time": bad_time, "settings": {}}]},
        expect_status=422,
    )
check(
    "curve mode BOOST",
    "POST",
    "/api/programs",
    {"name": "x", "kind": "curve", "curve": {"operational_mode": "BOOST"}},
    expect_status=422,
)
check(
    "curve fan 55",
    "POST",
    "/api/programs",
    {"name": "x", "kind": "curve", "curve": {"fan_speed": 55}},
    expect_status=422,
)
for temp in (15.5, 31.0):
    check(
        f"curve point {temp}",
        "POST",
        "/api/programs",
        {"name": "x", "kind": "curve",
         "curve": {"points": [{"time": "00:00", "temperature": temp}]}},
        expect_status=422,
    )
check(
    "third-degree setpoint",
    "POST",
    "/api/programs",
    {"name": "x", "favourite": {"target_temperature": 24.3}},
    expect_status=422,
)
check(
    "unknown fan in scene",
    "POST",
    "/api/programs",
    {"name": "x", "favourite": {"fan_speed": 55}},
    expect_status=422,
)

print("\n=== accepted input, and the normalising it does ===")
check("64-char name", "POST", "/api/programs", {"name": "a" * 64}, expect_status=201)
check("bare minimum", "POST", "/api/programs", {"name": "Minimal"}, expect_status=201)
check("favourite", "POST", "/api/programs", FAVOURITE, expect_status=201)
check("schedule (days sorted+deduped)", "POST", "/api/programs", SCHEDULE, expect_status=201)
check("curve (mode upper-cased)", "POST", "/api/programs", CURVE, expect_status=201)
check("empty curve defaults", "POST", "/api/programs",
      {"name": "Defaults", "kind": "curve", "curve": {}}, expect_status=201)
check("non-ascii name", "POST", "/api/programs", {"name": "Erkondišn ❄"}, expect_status=201)
check("list of everything", "GET", "/api/programs", expect_status=200)

print("\n=== apply, refusal, update, delete ===")
# Ids differ between the two servers, so these have to be resolved per server.
def id_of(impl, name):
    _, body = call(impl, "GET", "/api/programs")
    for p in json.loads(body):
        if p["name"] == name:
            return p["id"]
    return None


def check_per_server(label, method, path_fn, body=None, expect_status=None):
    sn, bn = call("native", method, path_fn("native"), body)
    sp, bp = call("python", method, path_fn("python"), body)
    cn, cp = canonical(bn), canonical(bp)
    if sn == sp and cn == cp and (expect_status is None or sn == expect_status):
        results["match"] += 1
        print(f"  match  [{sn}] {label}")
    else:
        results["differ"] += 1
        failures.append(f"{label}\n    native [{sn}]: {cn}\n    python [{sp}]: {cp}")
        print(f"  DIFFER {label}")


check_per_server(
    "apply a schedule is refused",
    "POST",
    lambda i: f"/api/programs/{id_of(i, 'Weekday')}/apply",
    expect_status=400,
)
check_per_server(
    "get one by id",
    "GET",
    lambda i: f"/api/programs/{id_of(i, 'Ljeto')}",
    expect_status=200,
)
check_per_server(
    "update keeps the id",
    "PUT",
    lambda i: f"/api/programs/{id_of(i, 'Ljeto')}",
    {"name": "Ljeto 2", "kind": "favourite", "favourite": {"power_state": False}},
    expect_status=200,
)
check_per_server(
    "delete returns 204",
    "DELETE",
    lambda i: f"/api/programs/{id_of(i, 'Minimal')}",
    expect_status=204,
)
check("list after the update and delete", "GET", "/api/programs", expect_status=200)

print("\n=== scheduler status shape ===")
sn, bn = call("native", "GET", "/api/programs/status")
sp, bp = call("python", "GET", "/api/programs/status")
kn = sorted(json.loads(bn).keys())
kp = sorted(json.loads(bp).keys())
if sn == sp == 200 and kn == kp:
    results["match"] += 1
    print(f"  match  [200] status keys: {kn}")
else:
    results["differ"] += 1
    failures.append(f"status: native [{sn}] {kn} vs python [{sp}] {kp}")
    print("  DIFFER status keys")
print(f"    native: {bn}")
print(f"    python: {bp}")

print("\n" + "=" * 62)
print(f"match {results['match']}  differ {results['differ']}  error {results['error']}")
if failures:
    print("\nFailures:")
    for f in failures:
        print(f"\n- {f}")
sys.exit(1 if results["differ"] or results["error"] else 0)

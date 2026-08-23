#!/usr/bin/env python3
"""Compare the unit-management endpoints and the v1→v2 upgrade, side by side.

These write to `config.json` and `devices.json`, so both servers must be pointed
at throwaway copies — see `tools/README.md`. Never run this against a live
deployment: it renames, adds and deletes units.

Covers what the general diff harness cannot, because each call changes state:

  PATCH /api/units/{id}      rename, and its validation
  POST  /api/units           add by address, which runs real discovery
  DELETE /api/units/{id}     remove
  POST  /api/auth/upgrade    move the calling device from a bearer token to
                             Ed25519, in place
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
NAMES = {8421: "native", 8422: "python"}

problems = []


def call(port, method, path, body=None):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(
        f"http://127.0.0.1:{port}{path}", data=data, method=method
    )
    req.add_header("X-API-Key", KEY)
    req.add_header("Authorization", f"Bearer {TOKENS[port]}")
    req.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(req, timeout=180) as r:
            raw = r.read().decode()
            return r.status, (json.loads(raw) if raw.strip() else None)
    except urllib.error.HTTPError as e:
        raw = e.read().decode()
        try:
            return e.code, json.loads(raw)
        except Exception:
            return e.code, raw[:200]
    except Exception as e:
        return None, f"transport: {e}"


def both(label, method, path, body=None, expect=None, compare_body=True):
    """Run the same call on both servers and compare."""
    results = {}
    for port in (8421, 8422):
        results[port] = call(port, method, path, body)
    (sn, bn), (sp, bp) = results[8421], results[8422]

    same = sn == sp and (not compare_body or bn == bp)
    if expect is not None and sn != expect:
        same = False
    if same:
        print(f"  same    [{sn}] {label}")
    else:
        print(f"  DIFFER  {label}")
        print(f"    native [{sn}]: {bn}")
        print(f"    python [{sp}]: {bp}")
        problems.append(label)
    return results


def unit_ids(port):
    _, config = call(port, "GET", "/api/config")
    return [u["id"] for u in (config or {}).get("units", [])]


print("=== the unit list starts identical ===")
native_units, python_units = unit_ids(8421), unit_ids(8422)
print(f"  native: {native_units}")
print(f"  python: {python_units}")
if native_units != python_units:
    problems.append("the two servers started from different unit lists")
    print("  cannot compare further")
    sys.exit(1)
target = native_units[0]

print("\n=== rename ===")
both("rename to a new name", "PATCH", f"/api/units/{target}", {"name": "Diff Test"}, expect=200)
both("rename validation: empty", "PATCH", f"/api/units/{target}", {"name": ""}, expect=422,
     compare_body=False)
both("rename validation: too long", "PATCH", f"/api/units/{target}",
     {"name": "a" * 65}, expect=422, compare_body=False)
both("rename an unknown unit", "PATCH", "/api/units/999", {"name": "x"}, expect=404)

print("\n=== add by address ===")
# The unit that was just renamed: re-adding it by its own address must keep its
# credentials and not duplicate it.
_, config = call(8421, "GET", "/api/config")
address = next(u["ip"] for u in config["units"] if u["id"] == target)

# Known to differ, and not a defect here: msmart's discover_single asks *Midea's
# cloud* for a V3 token as part of probing an address, and that call currently
# fails with "Code: 9999, Message: system error" -- so the reference answers 503
# for an address that is plainly reachable. This server does LAN discovery only:
# the unit itself supplies id, address, port and type, and V3 credentials stay
# whatever pairing already established. So it answers 201.
#
# The honest limit of that: adding a *brand new* V3 unit this way records its
# identity with no credentials (`has_v3_credentials: false`), and it cannot be
# controlled until a token and key are supplied. Identity from the LAN, secrets
# from pairing.
sn, bn = call(8421, "POST", "/api/units", {"ip": address})
sp, bp = call(8422, "POST", "/api/units", {"ip": address})
print(f"  known    native [{sn}] vs python [{sp}] -- add an address already known")
if sn != 201:
    problems.append(f"native add-by-address returned {sn}: {bn}")
    print(f"    native should have answered 201: {bn}")
else:
    print(f"    native added it: {bn.get('name')} ({bn.get('ip')})")
if sp == 503:
    print("    python failed as expected (its probe needs Midea's cloud)")
else:
    print(f"    python answered {sp}: {bp}")
both("add a bad address", "POST", "/api/units", {"ip": "not-an-ip"}, expect=422,
     compare_body=False)
both("add an address with nothing on it", "POST", "/api/units",
     {"ip": "192.168.199.201"}, expect=404, compare_body=False)

print("\n=== the lists still agree ===")
native_units, python_units = unit_ids(8421), unit_ids(8422)
if native_units == python_units:
    print(f"  same    {len(native_units)} unit(s), no duplicate from the re-add")
else:
    print(f"  DIFFER  native={native_units} python={python_units}")
    problems.append("unit lists diverged after add")

print("\n=== credentials survived the re-add ===")
for port in (8421, 8422):
    _, config = call(port, "GET", "/api/config")
    unit = next(u for u in config["units"] if u["id"] == target)
    state = "yes" if unit["has_v3_credentials"] else "NO"
    print(f"  {NAMES[port]}: has_v3_credentials={state}")
    if not unit["has_v3_credentials"]:
        problems.append(f"{NAMES[port]} lost credentials on re-add")

print("\n=== delete ===")
both("delete an unknown unit", "DELETE", "/api/units/999", expect=404)
both("delete a real unit", "DELETE", f"/api/units/{target}", expect=204)
native_units, python_units = unit_ids(8421), unit_ids(8422)
if native_units == python_units and target not in native_units:
    print(f"  same    it is gone from both; {len(native_units)} left")
else:
    print(f"  DIFFER  native={native_units} python={python_units}")
    problems.append("unit lists diverged after delete")

print("\n=== v1 -> v2 upgrade ===")
# Both device tokens are v1 bearers. Upgrading needs a real Ed25519 public key.
try:
    from Crypto.PublicKey import ECC
    import base64

    key = ECC.generate(curve="ed25519")
    public = (
        base64.urlsafe_b64encode(key.public_key().export_key(format="raw"))
        .decode()
        .rstrip("=")
    )
except ImportError:
    print("  skipped: pycryptodome not available in this interpreter")
    public = None

if public:
    both("a malformed public key is refused", "POST", "/api/auth/upgrade",
         {"public_key": "not-a-key"}, expect=400, compare_body=False)
    both("upgrade succeeds", "POST", "/api/auth/upgrade", {"public_key": public},
         expect=200, compare_body=False)
    for port in (8421, 8422):
        status, who = call(port, "GET", "/api/auth/whoami")
        # The bearer token was just invalidated, so this is expected to fail --
        # which is itself the check that the upgrade took effect.
        print(f"  {NAMES[port]}: whoami with the old bearer -> {status}")
        if status == 200:
            problems.append(f"{NAMES[port]} still accepts the retired bearer token")

print("\n" + "=" * 60)
if problems:
    print(f"{len(problems)} problem(s):")
    for p in problems:
        print(f"  - {p}")
    sys.exit(1)
print("the config API and the upgrade behave identically")

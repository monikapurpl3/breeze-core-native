#!/usr/bin/env python3
"""Pair an Ed25519 (auth_version 2) device and exercise it against both servers.

This is the case that matters most: the Android app authenticates this way. It
also caught a real bug — `whoami` re-verified the request inside the route, which
spends the v2 nonce a second time and rejects the caller as a replay. A v1 bearer
token never notices, so nothing before this had exercised it.

Signs exactly what the reference signs:

    breeze-auth-v2\\n{METHOD}\\n{path?query}\\n{timestamp}\\n{nonce}\\n{sha3_512(body) hex}
"""
import base64
import binascii
import hashlib
import json
import os
import sys
import time
import urllib.error
import urllib.request

from Crypto.Hash import SHA512
from Crypto.PublicKey import ECC
from Crypto.Signature import eddsa

KEY = open("/tmp/bcn.key").read().strip()
PREFIX = "breeze-auth-v2"


def b64u(raw):
    return base64.urlsafe_b64encode(raw).decode().rstrip("=")


class Device:
    """One paired v2 device, holding its key and signing every request."""

    def __init__(self, base):
        self.base = base
        self.key = ECC.generate(curve="ed25519")
        self.public_b64 = b64u(self.key.public_key().export_key(format="raw"))
        self.token_id = None

    def raw(self, method, path, body=None, headers=None):
        data = json.dumps(body).encode() if body is not None else None
        req = urllib.request.Request(self.base + path, data=data, method=method)
        req.add_header("X-API-Key", KEY)
        req.add_header("Content-Type", "application/json")
        for k, v in (headers or {}).items():
            req.add_header(k, v)
        try:
            with urllib.request.urlopen(req, timeout=60) as r:
                return r.status, r.read().decode()
        except urllib.error.HTTPError as e:
            return e.code, e.read().decode()

    def signed(self, method, path, body=None):
        """A v2-signed request: fresh nonce, current timestamp, real signature."""
        payload = json.dumps(body).encode() if body is not None else b""
        timestamp = str(int(time.time()))
        nonce = b64u(os.urandom(12))
        canonical = "\n".join(
            [PREFIX, method.upper(), path, timestamp, nonce,
             hashlib.sha3_512(payload).hexdigest()]
        ).encode()
        signer = eddsa.new(self.key, "rfc8032")
        signature = b64u(signer.sign(canonical))
        return self.raw(method, path, body, {
            "X-Breeze-Auth-Version": "2",
            "X-Breeze-Key-Id": self.token_id,
            "X-Breeze-Timestamp": timestamp,
            "X-Breeze-Nonce": nonce,
            "X-Breeze-Signature": signature,
        })

    def pair(self):
        status, body = self.raw("POST", "/api/auth/enroll/start", {
            "label": "v2-test", "auth_version": 2, "public_key": self.public_b64,
        })
        assert status == 200, f"start failed: {status} {body}"
        started = json.loads(body)
        status, body = self.raw("POST", "/api/auth/enroll/approve",
                                {"code": started["user_code"]})
        assert status == 200, f"approve failed: {status} {body}"
        status, body = self.raw("POST", "/api/auth/enroll/poll",
                                {"session_id": started["session_id"]})
        assert status == 200, f"poll failed: {status} {body}"
        polled = json.loads(body)
        assert polled["status"] == "approved", polled
        # A v2 device already holds its key, so no token is handed back.
        assert "device_token" not in polled, "a v2 enrolment must not mint a bearer token"
        assert polled["auth_version"] == 2, polled
        self.token_id = polled["token_id"]
        return polled


failures = []


def compare(label, native, python, key=None):
    """Compare two (status, body) pairs, optionally on one field."""
    sn, bn = native
    sp, bp = python
    if key:
        try:
            got = (sn, json.loads(bn).get(key))
            want = (sp, json.loads(bp).get(key))
        except Exception:
            got, want = (sn, bn), (sp, bp)
    else:
        got, want = (sn, bn), (sp, bp)
    if got == want:
        print(f"  same    [{sn}] {label}")
    else:
        failures.append(f"{label}\n    native: {sn} {bn[:300]}\n    python: {sp} {bp[:300]}")
        print(f"  DIFFER  {label}")


native = Device("http://127.0.0.1:8421")
python = Device("http://127.0.0.1:8422")

print("=== pairing an Ed25519 device with each server ===")
n_pair = native.pair()
p_pair = python.pair()
print(f"  native token_id {n_pair['token_id']}, auth_version {n_pair['auth_version']}")
print(f"  python token_id {p_pair['token_id']}, auth_version {p_pair['auth_version']}")

print("\n=== a signed request is accepted ===")
compare("GET /api/units", native.signed("GET", "/api/units"),
        python.signed("GET", "/api/units"))

print("\n=== whoami: the route that was broken for v2 ===")
n_who = native.signed("GET", "/api/auth/whoami")
p_who = python.signed("GET", "/api/auth/whoami")
print(f"  native [{n_who[0]}]: {n_who[1]}")
print(f"  python [{p_who[0]}]: {p_who[1]}")
if n_who[0] != 200:
    failures.append(f"whoami returned {n_who[0]} for a v2 device: {n_who[1]}")
    print("  FAIL    native whoami rejected a valid v2 request")
else:
    n_keys = list(json.loads(n_who[1]).keys())
    p_keys = list(json.loads(p_who[1]).keys())
    if n_keys == p_keys:
        print(f"  same    key order: {n_keys}")
    else:
        failures.append(f"whoami keys differ:\n    native {n_keys}\n    python {p_keys}")
        print("  DIFFER  whoami keys")
    n_body = json.loads(n_who[1])
    if n_body.get("auth_version") != 2:
        failures.append(f"whoami reported auth_version {n_body.get('auth_version')}")
    if n_body.get("last_used") is None:
        failures.append("whoami reported last_used=null on the request that used it")
        print("  FAIL    last_used is still null")
    else:
        print(f"  ok      last_used is set ({n_body['last_used']})")

print("\n=== a replayed nonce is still rejected ===")
# Same signed request twice: the second must be refused by both.
ts = str(int(time.time()))
nonce = b64u(os.urandom(12))


def replay(dev):
    canonical = "\n".join(
        [PREFIX, "GET", "/api/units", ts, nonce, hashlib.sha3_512(b"").hexdigest()]
    ).encode()
    sig = b64u(eddsa.new(dev.key, "rfc8032").sign(canonical))
    headers = {
        "X-Breeze-Auth-Version": "2",
        "X-Breeze-Key-Id": dev.token_id,
        "X-Breeze-Timestamp": ts,
        "X-Breeze-Nonce": nonce,
        "X-Breeze-Signature": sig,
    }
    first = dev.raw("GET", "/api/units", None, headers)
    second = dev.raw("GET", "/api/units", None, headers)
    return first, second


n_first, n_second = replay(native)
p_first, p_second = replay(python)
print(f"  native: first {n_first[0]}, replay {n_second[0]}")
print(f"  python: first {p_first[0]}, replay {p_second[0]}")
if n_first[0] != 200 or n_second[0] == 200:
    failures.append(f"native replay handling: first {n_first[0]}, second {n_second[0]}")
if (n_first[0], n_second[0]) != (p_first[0], p_second[0]):
    failures.append("replay status codes differ from the reference")

print("\n=== a tampered signature is rejected ===")
n_bad = native.raw("GET", "/api/units", None, {
    "X-Breeze-Auth-Version": "2",
    "X-Breeze-Key-Id": native.token_id,
    "X-Breeze-Timestamp": str(int(time.time())),
    "X-Breeze-Nonce": b64u(os.urandom(12)),
    "X-Breeze-Signature": b64u(b"\x00" * 64),
})
p_bad = python.raw("GET", "/api/units", None, {
    "X-Breeze-Auth-Version": "2",
    "X-Breeze-Key-Id": python.token_id,
    "X-Breeze-Timestamp": str(int(time.time())),
    "X-Breeze-Nonce": b64u(os.urandom(12)),
    "X-Breeze-Signature": b64u(b"\x00" * 64),
})
compare("forged signature", n_bad, p_bad, key="detail")

print("\n" + "=" * 62)
if failures:
    print(f"{len(failures)} problem(s):")
    for f in failures:
        print(f"\n- {f}")
    sys.exit(1)
print("v2 auth behaves identically on both servers")

// Auth v2 in the browser: an Ed25519 key pair the panel signs every request
// with, and which never leaves this browser.
//
// Until now the panel was the only client still on the v1 bearer token: the app
// has signed its requests since 3.0.0. This closes that gap, so the same server
// setting (AC_MIN_AUTH_VERSION=2) can lock out unsigned clients without locking
// out the web panel.
//
// Two things make this different from the app's implementation, both in the
// panel's favour:
//
//   * The private key is generated **non-extractable**. It is a CryptoKey handle
//     that WebCrypto will sign with and will not export — not even to the page
//     that created it. The app has to hold a seed it could in principle read
//     back; here there is nothing to read.
//   * It lives in IndexedDB, not localStorage. localStorage is strings only, so
//     storing a key there would mean making it extractable first, which is
//     exactly what we do not want. IndexedDB stores the CryptoKey itself.
//
// The wire format is the app's, to the byte: integer-second timestamp,
// 16 random bytes of nonce as unpadded base64url, unpadded base64url signature,
// raw 32-byte public key. See meow_ac/security/signing.py.

import { sha3_512_hex } from "./sha3.js";

const DB_NAME = "breeze-auth";
const DB_VERSION = 1;
const STORE = "signer";
const RECORD = "device";

// ---------------------------------------------------------------- encodings
const b64u = (bytes) => {
  let s = "";
  for (const b of bytes) s += String.fromCharCode(b);
  return btoa(s).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
};

// --------------------------------------------------------------- capability
let _supported = null;

/** Whether this browser can do Ed25519 at all. Cached — the answer cannot
 *  change within a page load, and generating a throwaway key to find out is not
 *  free. A browser without it keeps using the v1 bearer token, which still works
 *  unless the server is set to require v2. */
export async function ed25519Supported() {
  if (_supported !== null) return _supported;
  try {
    const pair = await crypto.subtle.generateKey({ name: "Ed25519" }, false, ["sign", "verify"]);
    _supported = !!pair.privateKey;
  } catch {
    _supported = false;
  }
  return _supported;
}

// ------------------------------------------------------------------ storage
function openDb() {
  return new Promise((resolve, reject) => {
    const req = indexedDB.open(DB_NAME, DB_VERSION);
    req.onupgradeneeded = () => {
      const db = req.result;
      if (!db.objectStoreNames.contains(STORE)) db.createObjectStore(STORE);
    };
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
  });
}

async function idb(mode, fn) {
  const db = await openDb();
  try {
    return await new Promise((resolve, reject) => {
      const tx = db.transaction(STORE, mode);
      const out = fn(tx.objectStore(STORE));
      tx.oncomplete = () => resolve(out?.result ?? null);
      tx.onerror = () => reject(tx.error);
      tx.onabort = () => reject(tx.error);
    });
  } finally {
    db.close();
  }
}

// -------------------------------------------------------------------- signer
/** Build the five v2 headers for one request.
 *
 *  Split out of the Signer class so it can be exercised without a browser:
 *  tools/v2-webauth-test.js imports THIS function, signs with a key it made in
 *  the same WebCrypto API, and has a real server verify the result. Testing a
 *  reimplementation would prove nothing about the code that ships.
 */
export async function signRequestHeaders({
  privateKey, keyId, method, path, body = "", clockOffsetSeconds = 0,
}) {
  const bodyBytes = new TextEncoder().encode(body ?? "");
  const ts = String(Math.floor(Date.now() / 1000) + clockOffsetSeconds);
  const nonceBytes = new Uint8Array(16);
  crypto.getRandomValues(nonceBytes);
  const nonce = b64u(nonceBytes);
  const canonical = new TextEncoder().encode(
    ["breeze-auth-v2", method.toUpperCase(), path, ts, nonce, sha3_512_hex(bodyBytes)].join("\n"),
  );
  const sig = new Uint8Array(
    await crypto.subtle.sign({ name: "Ed25519" }, privateKey, canonical),
  );
  return {
    "X-Breeze-Auth-Version": "2",
    "X-Breeze-Key-Id": keyId,
    "X-Breeze-Timestamp": ts,
    "X-Breeze-Nonce": nonce,
    "X-Breeze-Signature": b64u(sig),
  };
}

class Signer {
  constructor(keyId, privateKey, publicKeyB64) {
    this.keyId = keyId;
    this.privateKey = privateKey;
    this.publicKeyB64 = publicKeyB64;
    // Offset learned from a clock_skew rejection: requests are then stamped
    // with SERVER time. A browser on a machine whose clock is wrong would
    // otherwise fail every request and look like a dead credential — the exact
    // failure that cost the app its users' pairings before 2.1.1.
    this.clockOffsetSeconds = 0;
  }

  /** The five headers for one request. `path` must include the query string,
   *  because that is what the server signs over. */
  headers(method, path, bodyText) {
    return signRequestHeaders({
      privateKey: this.privateKey,
      keyId: this.keyId,
      method,
      path,
      body: bodyText,
      clockOffsetSeconds: this.clockOffsetSeconds,
    });
  }
}

/** Generate a fresh key pair. Not persisted yet: the key id only exists once
 *  the server has approved the enrolment, and a key stored before that would be
 *  an orphan if the user abandoned pairing. */
export async function generateSigner() {
  const pair = await crypto.subtle.generateKey({ name: "Ed25519" }, false, ["sign", "verify"]);
  const raw = new Uint8Array(await crypto.subtle.exportKey("raw", pair.publicKey));
  return { privateKey: pair.privateKey, publicKeyB64: b64u(raw) };
}

/** Persist a signer once the server has issued its key id. */
export async function persistSigner(keyId, privateKey, publicKeyB64) {
  await idb("readwrite", (store) =>
    store.put({ keyId, privateKey, publicKeyB64 }, RECORD));
  return new Signer(keyId, privateKey, publicKeyB64);
}

/** The stored signer, or null. Returns null rather than throwing if IndexedDB
 *  is unavailable (private windows on some browsers), so the panel falls back to
 *  the bearer token instead of failing to load. */
export async function loadSigner() {
  try {
    const rec = await idb("readonly", (store) => store.get(RECORD));
    if (!rec?.keyId || !rec?.privateKey) return null;
    return new Signer(rec.keyId, rec.privateKey, rec.publicKeyB64);
  } catch {
    return null;
  }
}

export async function clearSigner() {
  try {
    await idb("readwrite", (store) => store.delete(RECORD));
  } catch {/* nothing to clear */}
}

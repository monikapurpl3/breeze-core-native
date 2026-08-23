// api.js — the single HTTP client layer for the UI.
//
// LOAD-BEARING: every request to the backend goes through apiFetch(),
// which attaches BOTH credentials — the enrollment/API key (X-API-Key)
// and, once this device is paired, its per-device bearer token. A past
// UI rewrite bypassed this wrapper and shipped a panel that 401'd on
// every call. If you add new API calls, route them through apiFetch —
// never call fetch() directly.
//
// The enrollment key lives in localStorage:
//   meow_ac_key           the shared enrollment key (prompted for)
//
// The per-device credential is one of two things:
//   * auth v2 (preferred) — an Ed25519 key pair in IndexedDB, whose private
//     half is non-extractable: WebCrypto will sign with it and will not hand it
//     to the page. Every request carries a signature instead of a secret. See
//     signer.js.
//   * auth v1 (fallback)  — a bearer token in localStorage
//     (meow_ac_device_token), used when the browser has no Ed25519 or when the
//     device was paired before v2 existed.
//
// The panel was the last client still on v1; the app has signed its requests
// since Breeze Core 3.0.0. Now a server set to AC_MIN_AUTH_VERSION=2 can refuse
// unsigned clients without refusing its own web panel.
//
// apiFetch does not auto-clear either credential on 401; app.js/enroll.js own
// that decision, because a 401 can mean "wrong key" or "token expired" and the
// recovery differs.

import { loadSigner, clearSigner } from "./signer.js";

const KEY_STORAGE = "meow_ac_key";
const TOKEN_STORAGE = "meow_ac_device_token";

export function getApiKey(){
  let key = localStorage.getItem(KEY_STORAGE);
  if(!key){
    key = (prompt("API key (from /etc/meow-ac/config.json on meow):") || "").trim();
    if(key) localStorage.setItem(KEY_STORAGE, key);
  }
  return key;
}

export function clearApiKey(){
  localStorage.removeItem(KEY_STORAGE);
}

export function getDeviceToken(){
  return localStorage.getItem(TOKEN_STORAGE) || "";
}

export function setDeviceToken(token){
  localStorage.setItem(TOKEN_STORAGE, token);
}

export function clearDeviceToken(){
  localStorage.removeItem(TOKEN_STORAGE);
}

// The signer, looked up once per page load. `undefined` = not looked yet,
// `null` = this browser has no signing credential and should use the token.
let _signer;

async function currentSigner(){
  if(_signer === undefined) _signer = await loadSigner();
  return _signer;
}

/** Adopt a signer straight after enrolment, so the very next request is signed
 *  without waiting for a reload. */
export function adoptSigner(signer){ _signer = signer; }

/** Throw away the signing credential (a definitively rejected one). */
export async function forgetSigner(){
  _signer = null;
  await clearSigner();
}

export async function hasSigner(){ return !!(await currentSigner()); }

// What the server signs over is the path INCLUDING the query string, taken from
// the parsed URL. Deriving it here rather than trusting the caller's string
// means a relative path, or one with a query, still signs correctly — a
// mismatch would be a 401 with a "bad signature" that looks like a broken key.
function signablePath(path){
  const u = new URL(path, location.origin);
  return u.pathname + (u.search || "");
}

// Thin fetch wrapper: attaches the API key plus this device's credential —
// either five signature headers (v2) or the bearer token (v1). Returns the raw
// Response, so callers can still tell "needs pairing" (401) from other failures.
export async function apiFetch(path, opts = {}){
  return sendWithRetry(path, opts, true);
}

async function sendWithRetry(path, opts, mayRetry){
  const method = (opts.method || "GET").toUpperCase();
  const headers = Object.assign({}, opts.headers, {"X-API-Key": getApiKey()});
  const signer = await currentSigner();

  if(signer){
    const body = typeof opts.body === "string" ? opts.body : "";
    Object.assign(headers, await signer.headers(method, signablePath(path), body));
  }else{
    const token = getDeviceToken();
    if(token) headers["Authorization"] = "Bearer " + token;
  }

  const res = await fetch(path, Object.assign({}, opts, {headers}));
  if(res.status !== 401 || !signer || !mayRetry) return res;

  // A signed request can fail for two reasons that are nothing to do with the
  // credential, and both are fixable here rather than by making the user
  // re-pair. This is the same lesson the app learned the hard way in 2.1.1:
  // treating every 401 as "my credential is dead" cost people their pairing.
  //
  // res.clone() so the caller still gets an unread body if this turns out not
  // to be retryable. Safe on a 401: it is a short JSON error, never a stream.
  let detail = null;
  try{ detail = (await res.clone().json())?.detail; }catch{ /* not JSON */ }
  if(!detail?.retryable) return res;

  if(detail.error === "clock_skew" && typeof detail.server_time === "number"){
    // This machine's clock is outside the server's window. Learn the offset and
    // sign with SERVER time from now on, instead of failing every request.
    signer.clockOffsetSeconds = Math.round(detail.server_time - Date.now() / 1000);
    console.warn("breeze: clock is off by ~" + signer.clockOffsetSeconds + "s; using server time");
  }
  // Anything else retryable (a replayed nonce, from a request the browser
  // retried on its own) just needs a fresh nonce, which the retry generates.
  return sendWithRetry(path, opts, false);
}

// Consume a Server-Sent Events endpoint — deliberately over fetch(), NOT
// EventSource.
//
// EventSource cannot send custom request headers. It is the obvious tool for
// SSE and it is unusable here: this UI authenticates with *two* headers
// (X-API-Key and Authorization), so `new EventSource("/api/units/stream")`
// arrives unauthenticated and 401s forever, with no way to attach either
// credential. Some projects work around that by putting a token in the query
// string; that would end up in access logs and in the referrer, which is a poor
// trade for a secret that controls someone's house.
//
// So: fetch() with a streaming body reader, which keeps both headers and keeps
// every request going through this module, per the rule at the top of the file.
//
// Returns the Response so the caller can branch on status the same way it does
// for apiFetch — 401 means re-pair, 404 means an older server with no stream
// route and the caller should fall back to polling. onEvent is called with
// (eventName, dataString) per frame; keepalive comments are skipped.
export async function apiStream(path, { onEvent, onOpen, signal } = {}){
  const res = await apiFetch(path, {
    signal,
    headers: {"Accept": "text/event-stream"},
    cache: "no-store",
  });
  if(!res.ok || !res.body) return res;
  if(onOpen) onOpen();

  const reader = res.body.getReader();
  const decoder = new TextDecoder();
  let buf = "";
  for(;;){
    const { value, done } = await reader.read();
    if(done) break;
    // Normalise CRLF so frame splitting works regardless of what the proxy did.
    buf += decoder.decode(value, {stream: true}).replace(/\r\n/g, "\n");
    let cut;
    // A frame ends at a blank line. Anything still in buf is a partial frame —
    // a chunk boundary can land mid-frame, so it has to be carried over.
    while((cut = buf.indexOf("\n\n")) >= 0){
      const frame = buf.slice(0, cut);
      buf = buf.slice(cut + 2);
      let name = "message";
      const data = [];
      for(const line of frame.split("\n")){
        if(line === "" || line.startsWith(":")) continue;  // keepalive comment
        if(line.startsWith("event:")) name = line.slice(6).trim();
        else if(line.startsWith("data:")) data.push(line.slice(5).replace(/^ /, ""));
      }
      if(data.length && onEvent) onEvent(name, data.join("\n"));
    }
  }
  return res;
}

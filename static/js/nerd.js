// nerd.js — the "Nerd" panel: everything /api/system knows about this server,
// its host, its units and its enrolled devices.
//
// The mobile app has the same screen (7 taps on the version number). This is the
// web equivalent, and it exists for the same reason: when someone says "it's not
// working", the useful reply is a single screen they can copy and paste rather
// than a sequence of questions.
//
// ── Rendered generically, on purpose ────────────────────────────────────────
// This walks whatever JSON the server sends instead of naming fields. /api/system
// grows every time the server learns a new fact, and a hand-written renderer
// would silently stop showing the new ones — the opposite of what a diagnostics
// screen is for. The cost is that the labels are the server's key names, which
// for this audience is a feature.
//
// ── What it must never do ───────────────────────────────────────────────────
// /api/system deliberately omits the API key, device public keys and per-unit V3
// token/key (there is a server-side test asserting that). Since this renderer
// prints whatever it is given, it stays honest only as long as the endpoint does
// — so do not "enrich" this screen from other endpoints without checking what
// they return.

import { apiFetch } from "./api.js";

// The order /api/system itself uses, so the panel reads the way the server
// thinks. Taken from a live 3.1.0 snapshot rather than guessed — an earlier
// version of this list invented "host" and "python" sections that do not exist,
// which the generic renderer papered over by appending the real ones after them.
// Anything the server adds later that is not listed here still shows up, at the
// end; nothing is ever dropped.
const SECTION_ORDER = [
  "server", "os", "init", "cpu", "machine_uptime_seconds", "components",
  "network", "paths", "settings", "connection", "process", "units", "devices",
  "programs", "scheduler", "storage",
];

function fmtValue(v){
  if(v === null || v === undefined) return "—";
  if(typeof v === "boolean") return v ? "yes" : "no";
  if(Array.isArray(v)) return v.length ? v.join(", ") : "—";
  return String(v);
}

// Humanise a key for the row label without hiding what it really is.
function label(k){
  return k.replace(/_/g, " ");
}

function row(k, v){
  const r = document.createElement("div");
  r.className = "nerd-row";
  const kk = document.createElement("span");
  kk.className = "nerd-k";
  kk.textContent = label(k);
  const vv = document.createElement("span");
  vv.className = "nerd-v";
  vv.textContent = fmtValue(v);
  r.append(kk, vv);
  return r;
}

// Recursive: objects become nested blocks, arrays become one block per entry,
// scalars become rows. Depth only controls indentation.
function renderValue(container, key, value, depth){
  if(value !== null && typeof value === "object" && !Array.isArray(value)){
    const box = document.createElement("div");
    box.className = "nerd-sub";
    const h = document.createElement("div");
    h.className = "nerd-subtitle";
    h.textContent = label(key);
    box.appendChild(h);
    for(const [k, v] of Object.entries(value)) renderValue(box, k, v, depth + 1);
    container.appendChild(box);
    return;
  }
  if(Array.isArray(value) && value.some(x => x && typeof x === "object")){
    const box = document.createElement("div");
    box.className = "nerd-sub";
    const h = document.createElement("div");
    h.className = "nerd-subtitle";
    h.textContent = label(key) + " (" + value.length + ")";
    box.appendChild(h);
    value.forEach((entry, i) => {
      // Prefer a human handle over the index when the entry offers one.
      const name = entry && (entry.name || entry.id || entry.label);
      renderValue(box, name ? String(name) : "#" + (i + 1), entry, depth + 1);
    });
    container.appendChild(box);
    return;
  }
  container.appendChild(row(key, value));
}

function renderSnapshot(card, snap){
  // Known sections first, in a deliberate order; anything the server added that
  // this file has never heard of goes after, so new facts still surface.
  const keys = Object.keys(snap);
  const known = SECTION_ORDER.filter(k => keys.includes(k));
  const rest = keys.filter(k => !SECTION_ORDER.includes(k)).sort();
  for(const k of known.concat(rest)){
    const sec = document.createElement("div");
    sec.className = "nerd-section";
    const h = document.createElement("h3");
    h.textContent = label(k);
    sec.appendChild(h);
    const v = snap[k];
    if(v !== null && typeof v === "object" && !Array.isArray(v)){
      for(const [kk, vv] of Object.entries(v)) renderValue(sec, kk, vv, 1);
    }else{
      renderValue(sec, k, v, 1);
    }
    card.appendChild(sec);
  }
}

export function nerdDialog(){
  return new Promise((resolve) => {
    const overlay = document.createElement("div");
    overlay.className = "enroll-overlay";
    const card = document.createElement("div");
    card.className = "enroll-card nerd-card";
    overlay.appendChild(card);

    const h = document.createElement("h2");
    h.textContent = "Nerd";
    card.appendChild(h);

    const body = document.createElement("div");
    body.className = "nerd-body";
    const loading = document.createElement("p");
    loading.className = "dim";
    loading.textContent = "asking the server…";
    body.appendChild(loading);
    card.appendChild(body);

    const actions = document.createElement("div");
    actions.className = "enroll-actions";
    const copy = document.createElement("button");
    copy.className = "ghost-btn";
    copy.textContent = "Copy JSON";
    copy.disabled = true;
    const close = document.createElement("button");
    close.className = "ghost-btn";
    close.textContent = "Close";
    actions.append(copy, close);
    card.appendChild(actions);

    const done = () => { overlay.remove(); resolve(); };
    close.addEventListener("click", done);
    overlay.addEventListener("click", (e) => { if(e.target === overlay) done(); });
    document.addEventListener("keydown", function esc(e){
      if(e.key === "Escape"){ document.removeEventListener("keydown", esc); done(); }
    });

    document.body.appendChild(overlay);

    (async () => {
      let res;
      try{
        res = await apiFetch("/api/system");
      }catch(e){
        loading.textContent = "can't reach the server — " + e.message;
        return;
      }
      if(res.status === 404){
        // The UI ships with the server, so this should be impossible — unless a
        // proxy is fronting an older Breeze Core than the one serving this page.
        loading.textContent = "this server has no /api/system — it predates 3.0.5.";
        return;
      }
      if(!res.ok){
        loading.textContent = "can't read system info (" + res.status + ")";
        return;
      }
      const snap = await res.json();
      body.innerHTML = "";
      renderSnapshot(body, snap);

      const raw = JSON.stringify(snap, null, 2);
      copy.disabled = false;
      copy.addEventListener("click", async () => {
        try{
          await navigator.clipboard.writeText(raw);
          copy.textContent = "Copied";
        }catch(_){
          // Clipboard needs a secure context, and this panel is usually served
          // over plain http on the LAN — so it fails on exactly the deployment
          // it is most used on. Fall back to selecting the text so the user can
          // copy it by hand rather than being told nothing happened.
          const pre = document.createElement("pre");
          pre.className = "nerd-raw";
          pre.textContent = raw;
          body.prepend(pre);
          const sel = window.getSelection();
          const range = document.createRange();
          range.selectNodeContents(pre);
          sel.removeAllRanges();
          sel.addRange(range);
          copy.textContent = "selected — press Ctrl+C";
        }
        setTimeout(() => { copy.textContent = "Copy JSON"; }, 2500);
      });
    })();
  });
}

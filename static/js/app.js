// app.js — the entry module: ensures this device is paired, loads units,
// wires each card's control callback to the API, and keeps state live.
// It's the only module that combines transport (api.js), pairing
// (enroll.js), and rendering (unit-card.js).

import { apiFetch, apiStream, clearDeviceToken, forgetSigner } from "./api.js";
import { buildPanel, render, setError, setName } from "./unit-card.js";
import { enroll } from "./enroll.js";
import {
  addUnitDialog, renameDialog, confirmDialog, addSourceDialog, scanDialog,
  apiAddUnit, apiRenameUnit, apiDeleteUnit,
} from "./manage.js";
import { tempUnit, toggleTempUnit, beepEnabled, toggleBeep } from "./display.js";
import { initPalette, buildPalettePicker } from "./theme.js";
import { nerdDialog } from "./nerd.js";
import { programsDialog } from "./programs.js";

// Polling is now the FALLBACK, not the mechanism. The server pushes state on
// /api/units/stream; this interval only runs when that stream is unavailable
// (older server, proxy that buffers SSE, or a stream that keeps dropping).
const POLL_INTERVAL_MS = 5000;
const STREAM_RETRY_MS = 3000;      // backoff after a stream drops
const STREAM_RETRY_MAX_MS = 30000;
const panels = {}; // unit id -> panel object
let reauthing = false;
let streamAbort = null;    // AbortController for the live stream, if open
let streamRetry = STREAM_RETRY_MS;
let pollTimer = null;      // only non-null while falling back to polling

// A 401 that survived apiFetch's own retry means this device's credential is
// finished, whichever kind it is: clear it and re-run pairing. Both are dropped
// because only one of them is in use and the other is already absent — and
// leaving a stale signing key behind would have the panel keep signing with a
// key the server has forgotten.
//
// Guarded so concurrent 401s (one per panel) trigger a single pairing flow.
//
// Note what is NOT here: any decision about whether a 401 is fatal. apiFetch
// already retried the two rejections that are recoverable (a clock outside the
// server's window, a replayed nonce), so anything arriving here has been
// through that filter. Getting this distinction wrong is what cost the Android
// app its users' pairings before 2.1.1.
async function reauth(){
  if(reauthing) return;
  reauthing = true;
  clearDeviceToken();
  await forgetSigner();
  try{ await enroll(); }
  finally{ reauthing = false; }
}

async function control(p, body){
  if(p.pending) return;
  p.pending = true;
  try{
    // beep rides along on every control call, injected here rather than at each
    // call site — there are a dozen of those (mode, fan, swing, temp, power,
    // eco, turbo) and one of them would eventually be forgotten. The server
    // treats absent-or-false as silent, so sending it explicitly is harmless.
    const res = await apiFetch(`/api/units/${p.id}/control`, {
      method: "POST",
      headers: {"Content-Type": "application/json"},
      body: JSON.stringify(Object.assign({beep: beepEnabled()}, body))
    });
    if(res.status === 401){ setError(p, "session expired — re-pairing…"); reauth(); return; }
    if(!res.ok) throw new Error(await res.text());
    render(p, await res.json());
    setError(p, null);
  }catch(e){
    setError(p, "control failed — " + e.message);
  }finally{
    p.pending = false;
  }
}

// Poll every unit in ONE request via the batch endpoint (Breeze Core >= 2.4.0),
// fanned out server-side. Falls back to per-panel polling on an older server
// (batch route missing -> 404/405).
async function fetchAllStates(){
  let res;
  try{
    res = await apiFetch("/api/units/state");
  }catch(e){
    document.getElementById("globalStatus").textContent = "can't reach server — " + e.message;
    return;
  }
  if(res.status === 401){ reauth(); return; }
  if(res.status === 404 || res.status === 405){  // older server without the batch route
    Object.values(panels).forEach(fetchStateOne);
    return;
  }
  if(!res.ok){
    document.getElementById("globalStatus").textContent = "can't refresh (" + res.status + ")";
    return;
  }
  document.getElementById("globalStatus").textContent = "";
  const data = await res.json();
  for(const s of (data.states || [])){
    const p = panels[s.id];
    if(p){ render(p, s); setError(p, null); }
  }
  for(const err of (data.errors || [])){
    const p = panels[err.id];
    if(p) setError(p, "can't reach this unit — " + (err.detail || "offline"));
  }
}

// Single-unit fetch — fallback path and initial per-panel load.
async function fetchStateOne(p){
  try{
    const res = await apiFetch(`/api/units/${p.id}/state`);
    if(res.status === 401){ setError(p, "session expired — re-pairing…"); reauth(); return; }
    if(!res.ok) throw new Error(await res.text());
    render(p, await res.json());
    setError(p, null);
  }catch(e){
    setError(p, "can't reach this unit — " + e.message);
  }
}

// Fetch the unit list, driving the pairing flow on a 401 and retrying.
async function loadUnits(){
  while(true){
    let res;
    try{
      res = await apiFetch("/api/units");
    }catch(e){
      document.getElementById("globalStatus").textContent = "can't reach server — " + e.message;
      return null;
    }
    if(res.ok) return await res.json();
    if(res.status === 401){
      clearDeviceToken();
      await forgetSigner();
      await enroll();   // resolves once a credential is held
      continue;         // retry with it
    }
    document.getElementById("globalStatus").textContent = "can't load units (" + res.status + ")";
    return null;
  }
}

// Re-render every panel from its last state (used when the °C/°F unit flips).
function rerenderAll(){
  Object.values(panels).forEach(p => { if(p.state) render(p, p.state); });
}

// Per-card ⋮ actions: rename and remove.
function makeActions(){
  return {
    onRename: async (p) => {
      const cur = (p.state && p.state.name) || "";
      const r = await renameDialog(cur);
      if(!r || !r.name || r.name === cur) return;
      const res = await apiRenameUnit(p.id, r.name);
      if(res.status === 401){ reauth(); return; }
      if(!res.ok){ setError(p, "rename failed — " + await res.text()); return; }
      setName(p, r.name);
      if(p.state) p.state.name = r.name;
    },
    onRemove: async (p) => {
      const name = (p.state && p.state.name) || p.id;
      const ok = await confirmDialog({
        title: "Remove unit?",
        message: `"${name}" will be removed from the server config. Pair it again to re-add it.`,
        confirmLabel: "Remove",
      });
      if(!ok) return;
      const res = await apiDeleteUnit(p.id);
      if(res.status === 401){ reauth(); return; }
      if(!res.ok){ setError(p, "remove failed — " + await res.text()); return; }
      p.root.remove();
      delete panels[p.id];
      if(Object.keys(panels).length === 0){
        document.getElementById("emptyState").classList.remove("hidden");
      }
    },
  };
}

// (Re)build the grid from a unit list, replacing any existing panels.
function buildGrid(units){
  const grid = document.getElementById("grid");
  grid.innerHTML = "";
  Object.keys(panels).forEach(k => delete panels[k]);
  const empty = document.getElementById("emptyState");
  if(units.length === 0){ empty.classList.remove("hidden"); return; }
  empty.classList.add("hidden");
  const actions = makeActions();
  units.forEach(u => {
    const p = buildPanel(u, control, actions);
    panels[u.id] = p;
    grid.appendChild(p.root);
  });
}

async function reloadUnits(){
  const units = await loadUnits();
  if(units !== null) buildGrid(units);
}

// Add a unit — scan the network for it, or type its IP. Either way it ends
// at POST /api/units (the server discovers + writes config), then refresh.
async function doAddUnit(){
  const choice = await addSourceDialog();
  if(!choice) return;
  if(choice === "scan"){
    const picked = await scanDialog();
    if(!picked) return;
    if(picked === "__manual__") return addByIp();
    return addResolvedIp(picked, null);
  }
  return addByIp();
}

async function addByIp(){
  const r = await addUnitDialog();
  if(!r || !r.ip) return;
  return addResolvedIp(r.ip, r.name);
}

async function addResolvedIp(ip, name){
  const status = document.getElementById("globalStatus");
  status.textContent = "discovering unit…";
  const res = await apiAddUnit(ip, name);
  if(res.status === 401){ status.textContent = ""; reauth(); return; }
  if(!res.ok){ status.textContent = "add failed — " + await res.text(); return; }
  status.textContent = "";
  await reloadUnits();
  await fetchAllStates();
}

function wireHeader(){
  const actions = document.querySelector(".header-actions");
  if(actions) actions.prepend(buildPalettePicker());  // 🎨 Theme, first
  const toggle = document.getElementById("unitToggle");
  if(toggle){
    toggle.textContent = "°" + tempUnit();
    toggle.addEventListener("click", () => {
      toggle.textContent = "°" + toggleTempUnit();
      rerenderAll();
    });
  }
  const beep = document.getElementById("beepToggle");
  if(beep){
    const paint = (on) => {
      beep.textContent = on ? "🔔" : "🔕";
      beep.classList.toggle("on", on);
      beep.title = on ? "Units chirp on each command" : "Units stay silent";
    };
    paint(beepEnabled());
    beep.addEventListener("click", () => paint(toggleBeep()));
  }
  const progs = document.getElementById("programsBtn");
  if(progs) progs.addEventListener("click", () => {
    // Hand over the live panel state so "save a favourite from a unit" can use
    // what is actually on screen rather than re-fetching it.
    programsDialog(Object.values(panels).map(p => ({
      id: p.id,
      name: (p.state && p.state.name) || p.id,
      state: p.state,
    })));
  });
  const nerd = document.getElementById("nerdBtn");
  if(nerd) nerd.addEventListener("click", nerdDialog);

  const add = document.getElementById("addUnitBtn");
  if(add) add.addEventListener("click", doAddUnit);
  const emptyAdd = document.getElementById("emptyAddBtn");
  if(emptyAdd) emptyAdd.addEventListener("click", doAddUnit);
}

// Fill the page footer with the server's version + build commit.
async function loadVersion(){
  const el = document.getElementById("appFooter");
  if(!el) return;
  try{
    const res = await apiFetch("/api/version");
    if(!res.ok) return;
    const v = await res.json();
    el.textContent = "";
    const name = document.createElement("span");
    name.textContent = `${v.name || "Breeze Core"} v${v.version}`;
    el.appendChild(name);
    if(v.commit && v.commit !== "unknown"){
      el.appendChild(document.createTextNode(" · "));
      const c = document.createElement("code");
      c.textContent = v.commit;
      el.appendChild(c);
    }
  }catch(_){/* footer is best-effort */}
}

async function init(){
  initPalette();  // apply the saved colour palette before first paint
  wireHeader();   // header (theme, add unit, °C/°F) works even with zero units
  loadVersion();  // fill the footer (best-effort, independent of units)
  const units = await loadUnits();
  if(units === null) return;
  buildGrid(units);

  // One state read up front so the cards are populated immediately — the stream
  // only sends a unit when something changes, so waiting for it would leave the
  // panels blank until someone touched a remote.
  await fetchAllStates();

  document.addEventListener("visibilitychange", () => {
    if(document.hidden){
      // Closing the stream is not just tidiness. The server polls the units
      // centrally ONLY while at least one stream is open, so hanging up means a
      // backgrounded tab stops causing LAN traffic to the air conditioners
      // entirely — where the old 5s poll merely skipped ticks while leaving the
      // next one queued.
      stopLive();
    }else{
      fetchAllStates();   // catch up on whatever changed while hidden
      startLive();
    }
  });
  startLive();
}

// ── keeping state live ──────────────────────────────────────────────────────
// Preference order: the SSE stream, falling back to polling. The fallback is
// not decoration — a reverse proxy that buffers responses will happily accept
// the stream and then deliver nothing, and an older server has no stream route
// at all.

function stopPolling(){
  if(pollTimer){ clearInterval(pollTimer); pollTimer = null; }
}

function startPolling(why){
  if(pollTimer) return;
  console.info("breeze: live stream unavailable (" + why + ") — polling every " +
               (POLL_INTERVAL_MS / 1000) + "s");
  // Skip ticks while re-pairing: one tick would 401 once per panel, which is
  // enough to trip a server-side fail2ban jail.
  const tick = () => {
    if(reauthing || document.hidden) return;
    if(Object.keys(panels).length === 0) return;
    fetchAllStates();
  };
  pollTimer = setInterval(tick, POLL_INTERVAL_MS);
}

function stopLive(){
  if(streamAbort){ streamAbort.abort(); streamAbort = null; }
  stopPolling();
}

async function startLive(){
  if(streamAbort || document.hidden) return;
  const ctl = new AbortController();
  streamAbort = ctl;
  let opened = false;
  try{
    const res = await apiStream("/api/units/stream", {
      signal: ctl.signal,
      onOpen: () => {
        opened = true;
        streamRetry = STREAM_RETRY_MS;   // healthy connection resets the backoff
        stopPolling();                    // stream wins; stop double-reading
        setGlobalStatus("");
      },
      onEvent: (name, data) => {
        if(name !== "state") return;
        let s;
        try{ s = JSON.parse(data); }catch(_){ return; }
        const p = panels[s.id];
        if(p){ render(p, s); setError(p, null); }
      },
    });
    if(res.status === 401){ reauth(); return; }
    if(res.status === 404 || res.status === 405){
      // Older server: no stream route. Poll for the rest of the session and do
      // not keep retrying something that will never appear.
      streamAbort = null;
      startPolling("server has no /api/units/stream");
      return;
    }
    if(!res.ok){ throw new Error("HTTP " + res.status); }
    // Fell out of the read loop: the server closed the stream (restart, or a
    // proxy idle timeout). Reconnect.
  }catch(e){
    if(ctl.signal.aborted) return;        // we closed it on purpose
    if(!opened) startPolling(e.message);  // never got a frame — assume no SSE
  }
  streamAbort = null;
  if(document.hidden) return;
  // Reconnect with backoff, polling in the meantime so the UI is never frozen.
  startPolling("reconnecting");
  streamRetry = Math.min(streamRetry * 2, STREAM_RETRY_MAX_MS);
  setTimeout(startLive, streamRetry);
}

function setGlobalStatus(text){
  const el = document.getElementById("globalStatus");
  if(el) el.textContent = text;
}

init();

// manage.js — unit management: add-by-IP, rename, delete.
//
// Talks to the /api config-management endpoints (Breeze Core >= 2.2.0 for
// add/rename, >= 2.4.0 for delete) through apiFetch, so both credentials ride
// along. Dialogs are built in-DOM (CSP-safe: no inline styles/handlers) and
// reuse the .enroll-* overlay styles.

import { apiFetch } from "./api.js";

// A tiny modal: title + text fields, resolves to {key: value, …} or null.
// fields: [{ key, label, placeholder?, value? }]
function modal({ title, fields, submitLabel = "Save" }){
  return new Promise((resolve) => {
    const overlay = document.createElement("div");
    overlay.className = "enroll-overlay";

    const card = document.createElement("div");
    card.className = "enroll-card";
    overlay.appendChild(card);

    const h = document.createElement("h2");
    h.textContent = title;
    card.appendChild(h);

    const inputs = {};
    fields.forEach((f) => {
      const inp = document.createElement("input");
      inp.className = "enroll-input";
      inp.type = "text";
      inp.maxLength = 64;
      if(f.placeholder) inp.placeholder = f.placeholder;
      if(f.value) inp.value = f.value;
      card.appendChild(inp);
      inputs[f.key] = inp;
    });

    const err = document.createElement("div");
    err.className = "enroll-error hidden";
    card.appendChild(err);

    const btnRow = document.createElement("div");
    btnRow.className = "btn-row";
    const cancel = document.createElement("button");
    cancel.className = "enroll-btn secondary";
    cancel.textContent = "Cancel";
    const submit = document.createElement("button");
    submit.className = "enroll-btn";
    submit.textContent = submitLabel;
    btnRow.append(cancel, submit);
    card.appendChild(btnRow);

    const close = (result) => { overlay.remove(); resolve(result); };
    cancel.onclick = () => close(null);
    overlay.addEventListener("click", (e) => { if(e.target === overlay) close(null); });
    submit.onclick = () => {
      const out = {};
      for(const [k, inp] of Object.entries(inputs)) out[k] = inp.value.trim();
      close(out);
    };
    fields.forEach((f) => {
      inputs[f.key].addEventListener("keydown", (e) => { if(e.key === "Enter") submit.click(); });
    });

    document.body.appendChild(overlay);
    const first = inputs[fields[0].key];
    if(first) first.focus();
  });
}

// --- dialogs ---
export function addUnitDialog(){
  return modal({
    title: "Add unit by IP",
    submitLabel: "Add",
    fields: [
      { key: "ip", placeholder: "unit IP address (e.g. 192.168.1.73)" },
      { key: "name", placeholder: "name (optional)" },
    ],
  });
}
export function renameDialog(current){
  return modal({
    title: "Rename unit",
    submitLabel: "Save",
    fields: [{ key: "name", value: current || "", placeholder: "name" }],
  });
}

// A yes/no confirmation. Resolves true (confirmed) or false (cancelled).
export function confirmDialog({ title, message, confirmLabel = "OK" }){
  return new Promise((resolve) => {
    const overlay = document.createElement("div");
    overlay.className = "enroll-overlay";
    const card = document.createElement("div");
    card.className = "enroll-card";
    overlay.appendChild(card);

    const h = document.createElement("h2");
    h.textContent = title;
    card.appendChild(h);
    if(message){
      const p = document.createElement("p");
      p.className = "enroll-intro";
      p.textContent = message;
      card.appendChild(p);
    }

    const btnRow = document.createElement("div");
    btnRow.className = "btn-row";
    const cancel = document.createElement("button");
    cancel.className = "enroll-btn secondary";
    cancel.textContent = "Cancel";
    const ok = document.createElement("button");
    ok.className = "enroll-btn danger";
    ok.textContent = confirmLabel;
    btnRow.append(cancel, ok);
    card.appendChild(btnRow);

    const close = (r) => { overlay.remove(); resolve(r); };
    cancel.onclick = () => close(false);
    ok.onclick = () => close(true);
    overlay.addEventListener("click", (e) => { if(e.target === overlay) close(false); });
    document.body.appendChild(overlay);
    ok.focus();
  });
}

// Choose how to add a unit: resolves 'scan' | 'ip' | null.
export function addSourceDialog(){
  return new Promise((resolve) => {
    const overlay = document.createElement("div");
    overlay.className = "enroll-overlay";
    const card = document.createElement("div");
    card.className = "enroll-card";
    overlay.appendChild(card);

    const h = document.createElement("h2");
    h.textContent = "Add a unit";
    card.appendChild(h);

    const p = document.createElement("p");
    p.className = "enroll-intro";
    p.textContent = "Find units on your network automatically, or add one by its IP.";
    card.appendChild(p);

    const scan = document.createElement("button");
    scan.className = "enroll-btn";
    scan.textContent = "Scan the network";
    const manual = document.createElement("button");
    manual.className = "enroll-btn secondary";
    manual.textContent = "Enter IP address";
    card.append(scan, manual);

    const close = (r) => { overlay.remove(); resolve(r); };
    scan.onclick = () => close("scan");
    manual.onclick = () => close("ip");
    overlay.addEventListener("click", (e) => { if(e.target === overlay) close(null); });
    document.body.appendChild(overlay);
    scan.focus();
  });
}

// Scan the LAN and let the user pick a found unit. Resolves to a chosen IP,
// '__manual__' (user chose to type an IP), or null (cancelled). Handles a
// pre-3.0 server (no scan endpoint) by offering the manual path.
export function scanDialog(){
  return new Promise((resolve) => {
    const overlay = document.createElement("div");
    overlay.className = "enroll-overlay";
    const card = document.createElement("div");
    card.className = "enroll-card";
    overlay.appendChild(card);

    const h = document.createElement("h2");
    h.textContent = "Scan for units";
    card.appendChild(h);

    const list = document.createElement("div");
    list.className = "scan-list";
    card.appendChild(list);

    const btnRow = document.createElement("div");
    btnRow.className = "btn-row";
    const manual = document.createElement("button");
    manual.className = "enroll-btn secondary";
    manual.textContent = "Enter IP instead";
    const rescan = document.createElement("button");
    rescan.className = "enroll-btn secondary";
    rescan.textContent = "Rescan";
    const closeBtn = document.createElement("button");
    closeBtn.className = "enroll-btn secondary";
    closeBtn.textContent = "Close";
    btnRow.append(manual, rescan, closeBtn);
    card.appendChild(btnRow);

    const close = (r) => { overlay.remove(); resolve(r); };
    manual.onclick = () => close("__manual__");
    closeBtn.onclick = () => close(null);
    overlay.addEventListener("click", (e) => { if(e.target === overlay) close(null); });

    const setMsg = (text) => { list.textContent = ""; const p = document.createElement("p");
      p.className = "enroll-intro"; p.textContent = text; list.appendChild(p); };

    async function run(){
      setMsg("Scanning the network…");
      rescan.disabled = true;
      let res;
      try{ res = await apiScanUnits(); }
      catch(_){ setMsg("Scan failed — check the connection and try again."); rescan.disabled = false; return; }
      rescan.disabled = false;
      if(res.status === 404){ setMsg("This server is older than 3.0 and can’t scan. Add the unit by its IP instead."); return; }
      if(!res.ok){ setMsg("Scan failed (" + res.status + ")."); return; }
      const data = await res.json();
      const cands = (data.candidates || []);
      if(cands.length === 0){ setMsg("No units found. Make sure they’re powered on and on this network, or add one by its IP."); return; }
      list.textContent = "";
      for(const c of cands){
        const row = document.createElement("button");
        row.className = "enroll-btn secondary scan-row";
        row.disabled = !!c.known;
        row.textContent = c.known ? `${c.ip} — already added` : `${c.ip} — port ${c.port} open`;
        if(!c.known) row.onclick = () => close(c.ip);
        list.appendChild(row);
      }
    }
    rescan.onclick = run;
    document.body.appendChild(overlay);
    run();
  });
}

// --- API calls (return the raw Response; caller handles 401/errors) ---
export function apiScanUnits(){
  return apiFetch("/api/units/scan");
}
export function apiAddUnit(ip, name){
  const body = name ? { ip, name } : { ip };
  return apiFetch("/api/units", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
}
export function apiRenameUnit(id, name){
  return apiFetch(`/api/units/${id}`, {
    method: "PATCH",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ name }),
  });
}
export function apiDeleteUnit(id){
  return apiFetch(`/api/units/${id}`, { method: "DELETE" });
}

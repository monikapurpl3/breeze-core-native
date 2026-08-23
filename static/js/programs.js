// programs.js — favourites, schedules and curves (Breeze Core >= 3.0.0).
//
// Programs live server-side in programs.json and are fired by the server's own
// scheduler, so this panel is a *view onto server state*, not a client-side
// automation engine. That distinction matters: a schedule keeps working with
// every browser closed, which is the whole point of it living on the server.
//
// ── What this panel does and does not do ────────────────────────────────────
// Full read, apply, enable/disable and delete for all three kinds, plus creating
// a favourite from a unit's current state — the one creation path that is
// genuinely better here than in the app, because you set the unit up with the
// real controls and then save exactly what you are looking at.
//
// It does NOT edit schedule entries or curve points. Those need a repeating
// time/day editor that is a screen of its own, the mobile app already has one,
// and a half-built version here would be worse than sending people there. The
// panel says so rather than leaving the gap unexplained.
//
// ── One API asymmetry worth knowing ─────────────────────────────────────────
// POST /{id}/apply works for favourites (their scene) and curves (the current
// interpolated setpoint) but returns 400 for schedules, because a schedule is a
// set of future triggers with no single "now" scene. So Apply is not offered for
// schedules at all — better than offering a button that always errors.

import { apiFetch } from "./api.js";
import { confirmDialog } from "./manage.js";
import { fmtTemp } from "./display.js";

const DAYS = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];  // 0=Mon .. 6=Sun

function el(tag, cls, text){
  const e = document.createElement(tag);
  if(cls) e.className = cls;
  if(text !== undefined) e.textContent = text;
  return e;
}

function fanLabel(v){
  if(v === null || v === undefined) return null;
  return v === 102 ? "fan auto" : "fan " + v + "%";
}

// One-line description of a ControlRequest, skipping fields it doesn't set —
// a favourite is a partial scene, and printing "null" for the rest is noise.
function sceneSummary(s){
  if(!s) return "—";
  const bits = [];
  if(s.power_state === false) bits.push("off");
  else if(s.power_state === true) bits.push("on");
  if(s.operational_mode) bits.push(s.operational_mode.toLowerCase().replace("_", " "));
  if(s.target_temperature !== null && s.target_temperature !== undefined){
    bits.push(fmtTemp(s.target_temperature));
  }
  const fan = fanLabel(s.fan_speed);
  if(fan) bits.push(fan);
  if(s.swing_mode && s.swing_mode !== "OFF") bits.push("swing " + s.swing_mode.toLowerCase());
  if(s.eco) bits.push("eco");
  if(s.turbo) bits.push("turbo");
  return bits.length ? bits.join(" · ") : "no changes";
}

function daysLabel(days){
  if(!days || days.length === 0) return "every day";
  if(days.length === 7) return "every day";
  // Weekdays/weekend are common enough to be worth naming.
  const set = days.join(",");
  if(set === "0,1,2,3,4") return "weekdays";
  if(set === "5,6") return "weekends";
  return days.map(d => DAYS[d] || "?").join(" ");
}

function programSummary(p){
  if(p.kind === "favourite") return sceneSummary(p.favourite);
  if(p.kind === "schedule"){
    const n = (p.schedule || []).length;
    return n + (n === 1 ? " trigger" : " triggers");
  }
  if(p.kind === "curve"){
    const n = ((p.curve || {}).points || []).length;
    return n + (n === 1 ? " point" : " points") +
           (p.curve && p.curve.operational_mode ? " · " + p.curve.operational_mode.toLowerCase() : "");
  }
  return p.kind;
}

function unitsLabel(p, unitNames){
  if(!p.unit_ids || p.unit_ids.length === 0) return "all units";
  return p.unit_ids.map(id => unitNames[id] || id).join(", ");
}

export function programsDialog(units){
  const unitNames = {};
  (units || []).forEach(u => { unitNames[u.id] = u.name || u.id; });

  return new Promise((resolve) => {
    const overlay = el("div", "enroll-overlay");
    const card = el("div", "enroll-card nerd-card");
    overlay.appendChild(card);
    card.appendChild(el("h2", null, "Programs"));

    const status = el("p", "dim prog-status", "…");
    card.appendChild(status);

    const body = el("div", "nerd-body");
    body.appendChild(el("p", "dim", "loading…"));
    card.appendChild(body);

    const actions = el("div", "enroll-actions");
    const newFav = el("button", "ghost-btn", "+ Favourite from a unit");
    const close = el("button", "ghost-btn", "Close");
    actions.append(newFav, close);
    card.appendChild(actions);

    const done = () => { overlay.remove(); resolve(); };
    close.addEventListener("click", done);
    overlay.addEventListener("click", (e) => { if(e.target === overlay) done(); });
    document.addEventListener("keydown", function esc(e){
      if(e.key === "Escape"){ document.removeEventListener("keydown", esc); done(); }
    });
    document.body.appendChild(overlay);

    async function loadStatus(){
      try{
        const res = await apiFetch("/api/programs/status");
        if(!res.ok){ status.textContent = ""; return; }
        const s = await res.json();
        const bits = [];
        bits.push(s.running ? "scheduler running" : "scheduler stopped");
        if(s.tick_seconds) bits.push("tick " + s.tick_seconds + "s");
        if(s.programs !== undefined) bits.push(s.programs + " programs");
        status.textContent = bits.join(" · ") + " — times are the server's local time";
      }catch(_){ status.textContent = ""; }
    }

    async function refresh(){
      let res;
      try{
        res = await apiFetch("/api/programs");
      }catch(e){
        body.innerHTML = "";
        body.appendChild(el("p", "dim", "can't reach the server — " + e.message));
        return;
      }
      if(res.status === 404){
        body.innerHTML = "";
        body.appendChild(el("p", "dim", "this server has no /api/programs — it predates 3.0.0."));
        newFav.disabled = true;
        return;
      }
      if(!res.ok){
        body.innerHTML = "";
        body.appendChild(el("p", "dim", "can't load programs (" + res.status + ")"));
        return;
      }
      const list = await res.json();
      body.innerHTML = "";
      if(!list.length){
        body.appendChild(el("p", "dim",
          "No programs yet. Save a favourite from a unit below, or create schedules and curves in the Breeze app."));
        return;
      }
      for(const kind of ["favourite", "schedule", "curve"]){
        const of = list.filter(p => p.kind === kind);
        if(!of.length) continue;
        const sec = el("div", "nerd-section");
        sec.appendChild(el("h3", null, kind === "favourite" ? "favourites" : kind + "s"));
        of.forEach(p => sec.appendChild(programRow(p)));
        body.appendChild(sec);
      }
    }

    function programRow(p){
      const box = el("div", "prog-item");
      const head = el("div", "prog-head");
      const title = el("span", "prog-name", p.name);
      if(!p.enabled) title.classList.add("prog-off");
      head.appendChild(title);
      head.appendChild(el("span", "prog-meta", unitsLabel(p, unitNames)));
      box.appendChild(head);
      box.appendChild(el("div", "prog-summary", programSummary(p)));

      // Schedule entries are worth showing in full: "when" is the entire point.
      if(p.kind === "schedule" && (p.schedule || []).length){
        const ul = el("div", "prog-entries");
        p.schedule.forEach(e => {
          ul.appendChild(el("div", "prog-entry",
            e.time + "  " + daysLabel(e.days) + "  →  " + sceneSummary(e.settings)));
        });
        box.appendChild(ul);
      }
      if(p.kind === "curve" && p.curve && (p.curve.points || []).length){
        const ul = el("div", "prog-entries");
        p.curve.points.forEach(pt => {
          ul.appendChild(el("div", "prog-entry", pt.time + "  →  " + fmtTemp(pt.temperature)));
        });
        box.appendChild(ul);
      }

      const row = el("div", "prog-actions");
      // Apply: favourites and curves only — the server 400s for schedules, and a
      // button that always fails is worse than no button.
      if(p.kind !== "schedule"){
        const apply = el("button", "ghost-btn", "Apply now");
        apply.addEventListener("click", async () => {
          apply.disabled = true;
          apply.textContent = "applying…";
          const r = await apiFetch(`/api/programs/${p.id}/apply`, {method: "POST"});
          apply.textContent = r.ok ? "applied" : "failed (" + r.status + ")";
          setTimeout(() => { apply.textContent = "Apply now"; apply.disabled = false; }, 2000);
        });
        row.appendChild(apply);
      }

      const toggle = el("button", "ghost-btn", p.enabled ? "Disable" : "Enable");
      toggle.addEventListener("click", async () => {
        toggle.disabled = true;
        // PUT takes a whole ProgramSpec, so send the program back with `enabled`
        // flipped. Sending only {enabled} would blank every other field.
        const spec = Object.assign({}, p, {enabled: !p.enabled});
        delete spec.id;
        const r = await apiFetch(`/api/programs/${p.id}`, {
          method: "PUT",
          headers: {"Content-Type": "application/json"},
          body: JSON.stringify(spec),
        });
        toggle.disabled = false;
        if(r.ok){ await refresh(); loadStatus(); }
        else toggle.textContent = "failed (" + r.status + ")";
      });
      row.appendChild(toggle);

      const del = el("button", "ghost-btn", "Delete");
      del.addEventListener("click", async () => {
        const ok = await confirmDialog({
          title: "Delete program?",
          message: `"${p.name}" will be removed from the server. This can't be undone.`,
          confirmLabel: "Delete",
        });
        if(!ok) return;
        const r = await apiFetch(`/api/programs/${p.id}`, {method: "DELETE"});
        if(r.ok || r.status === 204){ await refresh(); loadStatus(); }
      });
      row.appendChild(del);

      box.appendChild(row);
      return box;
    }

    // Save what a unit is doing right now as a favourite. This is the creation
    // path worth having in a browser: you dial the unit in with the real
    // controls, then keep exactly that.
    newFav.addEventListener("click", async () => {
      const withState = (units || []).filter(u => u.state);
      if(!withState.length){
        status.textContent = "no unit state yet — wait for the first refresh";
        return;
      }
      const chosen = await pickUnit(withState);
      if(!chosen) return;
      const name = (prompt(`Name for this favourite (from "${chosen.name}"):`) || "").trim();
      if(!name) return;
      const s = chosen.state;
      const spec = {
        name,
        kind: "favourite",
        enabled: true,
        unit_ids: [chosen.id],
        favourite: {
          power_state: s.power,
          operational_mode: s.operational_mode,
          target_temperature: s.target_temperature,
          fan_speed: s.fan_speed,
          swing_mode: s.swing_mode,
          eco: s.eco_mode,
          turbo: s.turbo_mode,
        },
      };
      const r = await apiFetch("/api/programs", {
        method: "POST",
        headers: {"Content-Type": "application/json"},
        body: JSON.stringify(spec),
      });
      if(r.ok || r.status === 201){ await refresh(); loadStatus(); }
      else status.textContent = "couldn't save (" + r.status + ") — " + await r.text();
    });

    loadStatus();
    refresh();
  });
}

// Small chooser reusing the modal look; returns the picked unit or null.
function pickUnit(units){
  return new Promise((resolve) => {
    const overlay = el("div", "enroll-overlay");
    const card = el("div", "enroll-card");
    overlay.appendChild(card);
    card.appendChild(el("h2", null, "Which unit?"));
    card.appendChild(el("p", "dim", "Its current settings become the favourite."));
    const list = el("div", "prog-pick");
    units.forEach(u => {
      const b = el("button", "ghost-btn", (u.name || u.id) + " — " + sceneSummary({
        power_state: u.state.power,
        operational_mode: u.state.operational_mode,
        target_temperature: u.state.target_temperature,
        fan_speed: u.state.fan_speed,
      }));
      b.addEventListener("click", () => { overlay.remove(); resolve(u); });
      list.appendChild(b);
    });
    card.appendChild(list);
    const actions = el("div", "enroll-actions");
    const cancel = el("button", "ghost-btn", "Cancel");
    cancel.addEventListener("click", () => { overlay.remove(); resolve(null); });
    actions.appendChild(cancel);
    card.appendChild(actions);
    document.body.appendChild(overlay);
  });
}

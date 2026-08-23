// unit-card.js — everything about one unit's panel: DOM construction,
// event wiring, and rendering a state object into it.
//
// It knows nothing about the network. It's handed a `control(panel,
// body)` callback and calls that when the user touches something; app.js
// supplies the callback that actually talks to the API. That split is
// what lets you restyle/extend a card here without touching transport,
// and vice versa.

import { nextSwingMode } from "./swing.js";
import { fmtTemp } from "./display.js";

const DIAL_CIRC = 2 * Math.PI * 78;

// buildPanel(unit, control, actions) -> panel object { root, refs, id, state, pending }
// `control` is the callback invoked as control(panel, body) on any input.
// `actions` = { onRename(panel), onRemove(panel) } wires the ⋮ menu.
export function buildPanel(unit, control, actions = {}){
  const tpl = document.getElementById("panelTemplate");
  const node = tpl.content.firstElementChild.cloneNode(true);

  const refs = {};
  node.querySelectorAll("[data-role]").forEach(el => {
    refs[el.dataset.role] = el;
  });
  refs.modePills = Array.from(node.querySelectorAll("[data-role=modeRow] .pill"));
  refs.fanPills = Array.from(node.querySelectorAll("[data-role=fanRow] .pill"));

  refs.name.textContent = unit.name;

  const p = { root: node, refs, id: unit.id, state: null, pending: false };

  refs.powerSwitch.addEventListener("click", () => {
    if(!p.state) return;
    control(p, {power_state: !p.state.power_state});
  });
  refs.ecoSwitch.addEventListener("click", () => {
    if(!p.state) return;
    control(p, {eco: !p.state.eco});
  });
  refs.turboSwitch.addEventListener("click", () => {
    if(!p.state) return;
    control(p, {turbo: !p.state.turbo});
  });
  refs.vSwitch.addEventListener("click", () => {
    if(!p.state) return;
    control(p, {swing_mode: nextSwingMode(p.state.swing_mode, "v")});
  });
  refs.hSwitch.addEventListener("click", () => {
    if(!p.state) return;
    control(p, {swing_mode: nextSwingMode(p.state.swing_mode, "h")});
  });
  refs.tempUp.addEventListener("click", () => {
    if(!p.state) return;
    control(p, {target_temperature: Math.min(30, p.state.target_temperature + 0.5)});
  });
  refs.tempDown.addEventListener("click", () => {
    if(!p.state) return;
    control(p, {target_temperature: Math.max(16, p.state.target_temperature - 0.5)});
  });
  refs.modePills.forEach(pill => {
    pill.addEventListener("click", () => control(p, {operational_mode: pill.dataset.mode}));
  });
  refs.fanPills.forEach(pill => {
    pill.addEventListener("click", () => control(p, {fan_speed: Number(pill.dataset.fan)}));
  });

  // ⋮ menu: rename / remove. The menu closes on outside click or Escape.
  if(refs.menuBtn && refs.menu){
    const closeMenu = () => refs.menu.classList.add("hidden");
    refs.menuBtn.addEventListener("click", (e) => {
      e.stopPropagation();
      refs.menu.classList.toggle("hidden");
    });
    document.addEventListener("click", closeMenu);
    document.addEventListener("keydown", (e) => { if(e.key === "Escape") closeMenu(); });
    if(refs.renameBtn){
      refs.renameBtn.addEventListener("click", () => { closeMenu(); actions.onRename && actions.onRename(p); });
    }
    if(refs.removeBtn){
      refs.removeBtn.addEventListener("click", () => { closeMenu(); actions.onRemove && actions.onRemove(p); });
    }
  }

  return p;
}

// Update the displayed unit name (after a rename).
export function setName(p, name){
  p.refs.name.textContent = name;
}

export function setError(p, msg){
  const box = p.refs.errorBox;
  if(msg){ box.textContent = msg; box.style.display = "block"; }
  else{ box.style.display = "none"; }
}

export function render(p, s){
  p.state = s;
  const r = p.refs;
  p.root.setAttribute("data-mode", s.operational_mode);

  r.dot.className = "dot" + (s.online ? " online" : "");
  r.statusText.textContent = s.online ? "online" : "offline";

  r.indoorTemp.textContent = fmtTemp(s.indoor_temperature, {showUnit: false});
  r.targetReadout.textContent = fmtTemp(s.target_temperature, {showUnit: false});
  r.targetBig.textContent = fmtTemp(s.target_temperature, {showUnit: false});

  const lo = 16, hi = 30;
  const frac = Math.min(1, Math.max(0, (s.target_temperature - lo) / (hi - lo)));
  r.dialFill.style.strokeDasharray = DIAL_CIRC;
  r.dialFill.style.strokeDashoffset = DIAL_CIRC * (1 - frac);

  r.powerSwitch.className = "switch power-switch" + (s.power_state ? " on" : "");
  r.ecoSwitch.className = "switch" + (s.eco ? " on" : "");
  r.turboSwitch.className = "switch" + (s.turbo ? " on" : "");
  r.vSwitch.className = "switch" + (s.swing_mode === "VERTICAL" || s.swing_mode === "BOTH" ? " on" : "");
  r.hSwitch.className = "switch" + (s.swing_mode === "HORIZONTAL" || s.swing_mode === "BOTH" ? " on" : "");

  r.modePills.forEach(pill => pill.classList.toggle("active", pill.dataset.mode === s.operational_mode));
  r.fanPills.forEach(pill => pill.classList.toggle("active", Number(pill.dataset.fan) === s.fan_speed));

  r.footer.textContent = "updated " + new Date().toLocaleTimeString();
}

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
import { buildClimateBar } from "./climate-bar.js";
import { sleepText, startText } from "./timers.js";
import { announce } from "./a11y.js";

const DIAL_CIRC = 2 * Math.PI * 78;

// buildPanel(unit, control, actions) -> panel object { root, refs, id, state, pending }
// `control` is the callback invoked as control(panel, body) on any input.
// `actions` = { onRename(panel), onRemove(panel) } wires the ⋮ menu, and
// { onTimer(panel), onCancelTimer(panel, kind) } the timer row.
export function buildPanel(unit, control, actions = {}){
  const tpl = document.getElementById("panelTemplate");
  const node = tpl.content.firstElementChild.cloneNode(true);

  const refs = {};
  node.querySelectorAll("[data-role]").forEach(el => {
    refs[el.dataset.role] = el;
  });
  refs.modePills = Array.from(node.querySelectorAll("[data-role=modeRow] .pill"));
  refs.fanPills = Array.from(node.querySelectorAll("[data-role=fanRow] .pill"));

  // The card is a <section> named by its heading, so a screen reader says
  // "Living Room, region" on the way in and every control in it has that
  // context; headings also let one jump from unit to unit.
  refs.name.id = "unit-" + unit.id + "-name";
  node.setAttribute("aria-labelledby", refs.name.id);
  refs.name.textContent = unit.name;
  refs.menuBtn.setAttribute("aria-label", "Options for " + unit.name);

  // The indoor / outdoor / target bar, built rather than templated because it
  // owns its own geometry and there is nothing for the HTML to say about it.
  const climate = buildClimateBar();
  refs.dialWrap = node.querySelector(".dial-wrap");
  refs.dialWrap.after(climate.el);

  const p = { root: node, refs, id: unit.id, state: null, pending: false, climate };

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

  // The timer row: the button opens the dialog, each chip's × cancels its own.
  refs.timerBtn.addEventListener("click", () => actions.onTimer && actions.onTimer(p));
  refs.sleepCancel.addEventListener("click", () => actions.onCancelTimer && actions.onCancelTimer(p, "sleep"));
  refs.startCancel.addEventListener("click", () => actions.onCancelTimer && actions.onCancelTimer(p, "start"));

  // ⋮ menu: rename / remove. The menu closes on outside click or Escape.
  // aria-expanded says whether it is open; opening moves focus to its first
  // item, and Escape from inside it puts focus back on the ⋮.
  if(refs.menuBtn && refs.menu){
    const closeMenu = () => {
      refs.menu.classList.add("hidden");
      refs.menuBtn.setAttribute("aria-expanded", "false");
    };
    refs.menuBtn.addEventListener("click", (e) => {
      e.stopPropagation();
      const opening = refs.menu.classList.contains("hidden");
      refs.menu.classList.toggle("hidden", !opening);
      refs.menuBtn.setAttribute("aria-expanded", String(opening));
      if(opening && refs.renameBtn) refs.renameBtn.focus();
    });
    document.addEventListener("click", closeMenu);
    document.addEventListener("keydown", (e) => {
      if(e.key !== "Escape") return;
      const inside = refs.menu.contains(document.activeElement);
      closeMenu();
      if(inside) refs.menuBtn.focus();
    });
    if(refs.renameBtn){
      refs.renameBtn.addEventListener("click", () => { closeMenu(); actions.onRename && actions.onRename(p); });
    }
    if(refs.removeBtn){
      refs.removeBtn.addEventListener("click", () => { closeMenu(); actions.onRemove && actions.onRemove(p); });
    }
  }

  return p;
}

// The unit's timers, as the server last listed them: { sleep, start }, each a
// timer or null, and when that list was fetched (the countdown runs from it).
export function setTimers(p, timers, fetchedAt){
  p.timers = timers;
  p.timersFetchedAt = fetchedAt;
  renderTimers(p);
}

// Redraw the chips. Called on every new list, and on a slow tick in between so
// "off in 42 min" keeps counting without asking the server again.
export function renderTimers(p){
  const r = p.refs;
  const t = p.timers || {};
  r.sleepChip.classList.toggle("hidden", !t.sleep);
  r.startChip.classList.toggle("hidden", !t.start);
  if(t.sleep) r.sleepText.textContent = "⏳ " + sleepText(t.sleep, p.timersFetchedAt);
  if(t.start) r.startText.textContent = "⏰ " + startText(t.start, p.timersFetchedAt);
}

// Update the displayed unit name (after a rename).
export function setName(p, name){
  p.refs.name.textContent = name;
  p.refs.menuBtn.setAttribute("aria-label", "Options for " + name);
}

// Said aloud when a message first appears, with the unit's name: the polls
// set the same error again every few seconds while a unit is unreachable,
// and that must not be read every few seconds.
function speakOnce(p, slot, msg){
  if(msg && msg !== p[slot]) announce(p.refs.name.textContent + ": " + msg);
  p[slot] = msg || null;
}

export function setError(p, msg){
  const box = p.refs.errorBox;
  speakOnce(p, "spokenError", msg);
  if(msg){ box.textContent = msg; box.style.display = "block"; }
  else{ box.style.display = "none"; }
}

// A change the unit refused. Clears itself: it is about one tap, and a notice
// that outlives the situation it describes is noise on the next look.
const NOTICE_MS = 15000;
export function setNotice(p, msg){
  const box = p.refs.noticeBox;
  clearTimeout(p.noticeTimer);
  speakOnce(p, "spokenNotice", msg);
  if(msg){
    box.textContent = msg;
    box.style.display = "block";
    p.noticeTimer = setTimeout(() => { box.style.display = "none"; }, NOTICE_MS);
  }else{
    box.style.display = "none";
  }
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

  // Each switch's state goes to the button (aria-checked, which is what is
  // read out) and to the knob drawn inside it (the class, which is what is
  // seen). The power switch is its own knob.
  const sw = (button, knob, on) => {
    button.setAttribute("aria-checked", String(!!on));
    knob.classList.toggle("on", !!on);
  };
  sw(r.powerSwitch, r.powerSwitch, s.power_state);
  sw(r.ecoSwitch, r.ecoKnob, s.eco);
  sw(r.turboSwitch, r.turboKnob, s.turbo);
  sw(r.vSwitch, r.vKnob, s.swing_mode === "VERTICAL" || s.swing_mode === "BOTH");
  sw(r.hSwitch, r.hKnob, s.swing_mode === "HORIZONTAL" || s.swing_mode === "BOTH");

  // The chosen mode and fan speed: aria-pressed for the ear, .active for the eye.
  const choose = (pill, on) => {
    pill.classList.toggle("active", on);
    pill.setAttribute("aria-pressed", String(on));
  };
  r.modePills.forEach(pill => choose(pill, pill.dataset.mode === s.operational_mode));
  r.fanPills.forEach(pill => choose(pill, Number(pill.dataset.fan) === s.fan_speed));

  // Indoor, outdoor and target in one picture, by the same rule the app uses.
  p.climate.update(s);

  r.footer.textContent = "updated " + new Date().toLocaleTimeString();
}

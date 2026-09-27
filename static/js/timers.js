// timers.js — a unit's two timers: the sleep timer ("off in 45 minutes") and
// the scheduled start ("on in 3 days at 07:30", Breeze Core 4.2.0).
//
// Every moment is the server's to decide. A sleep timer is asked for in
// minutes; a start as a number of days from the server's today plus an HH:MM on
// the server's clock. Countdowns run from the server's seconds_remaining,
// counted down locally from when it was fetched — this browser's clock is used
// only to measure elapsed time, never to say what time it is.
//
// The API calls go through apiFetch, like everything else, so both credentials
// ride along. The dialog is built in-DOM (CSP-safe) from the same .enroll-*
// pieces as the other dialogs.

import { apiFetch } from "./api.js";

export const SLEEP_PRESETS = [15, 30, 45, 60, 90, 120];
export const MAX_SLEEP_MINUTES = 24 * 60;
export const MAX_START_DAYS = 30;

// ── API ─────────────────────────────────────────────────────────────────────

export function apiListTimers(){
  // ?kind=all: without it, a 4.2.0 server lists only sleep timers, so that a
  // client from before scheduled starts never mistakes one for a sleep timer.
  return apiFetch("/api/timers?kind=all");
}

export function apiSleepTimer(unitId, minutes){
  return apiFetch("/api/timers", {
    method: "POST",
    headers: {"Content-Type": "application/json"},
    body: JSON.stringify({unit_ids: [String(unitId)], minutes}),
  });
}

export function apiStartTimer(unitId, days, at){
  return apiFetch("/api/timers", {
    method: "POST",
    headers: {"Content-Type": "application/json"},
    body: JSON.stringify({unit_ids: [String(unitId)], days, at}),
  });
}

export function apiCancelTimer(timerId){
  return apiFetch(`/api/timers/${encodeURIComponent(timerId)}`, {method: "DELETE"});
}

// ── reading the list ────────────────────────────────────────────────────────

// The unit's sleep timer and scheduled start, either possibly null. A timer
// with no unit ids covers every unit. A server older than 4.2.0 sends no
// `kind`; everything it has is a sleep timer.
export function timersFor(list, unitId){
  const id = String(unitId);
  const covers = (t) => !t.unit_ids || t.unit_ids.length === 0 || t.unit_ids.map(String).includes(id);
  const mine = (list || []).filter(covers);
  return {
    sleep: mine.find((t) => (t.kind || "sleep") === "sleep") || null,
    start: mine.find((t) => t.kind === "start") || null,
  };
}

// Seconds left now, from the server's figure and how long ago it was fetched.
export function remaining(t, fetchedAt){
  const elapsed = Math.floor((Date.now() - fetchedAt) / 1000);
  return Math.max(0, (t.seconds_remaining || 0) - elapsed);
}

// "42 min", "1 h 5 min", "less than a minute".
export function durationText(seconds){
  const minutes = Math.ceil(seconds / 60);
  if(seconds <= 0) return "a moment";
  if(minutes < 1) return "less than a minute";
  if(minutes < 60) return `${minutes} min`;
  const h = Math.floor(minutes / 60), m = minutes % 60;
  return m ? `${h} h ${m} min` : `${h} h`;
}

const WEEKDAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

// The server-local wall time in fires_at ("2026-09-30T07:30:00"), read as the
// calendar date and clock time it names — no timezone conversion, because it
// is the server's time and is shown as the server's time.
function wallClock(iso){
  const m = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})/.exec(iso || "");
  if(!m) return null;
  return {y: +m[1], mo: +m[2], d: +m[3], hh: m[4], mm: m[5]};
}

// Days between two {y, mo, d} dates, whatever the timezone.
function dayDiff(a, b){
  return Math.round((Date.UTC(b.y, b.mo - 1, b.d) - Date.UTC(a.y, a.mo - 1, a.d)) / 86400000);
}

// The server's "today", recovered from the timer itself: fires_at minus the
// seconds the server said were left when it answered.
function serverToday(t, fetchedAt){
  const w = wallClock(t.fires_at);
  if(!w) return null;
  const fires = Date.UTC(w.y, w.mo - 1, w.d, +w.hh, +w.mm);
  const now = new Date(fires - remaining(t, fetchedAt) * 1000);
  return {y: now.getUTCFullYear(), mo: now.getUTCMonth() + 1, d: now.getUTCDate()};
}

// "today at 18:05", "tomorrow at 07:30", "Tue 30 Sep at 07:30".
export function startWhenText(t, fetchedAt){
  const w = wallClock(t.fires_at);
  if(!w) return "at an unknown time";
  const time = `${w.hh}:${w.mm}`;
  const today = serverToday(t, fetchedAt);
  const diff = today ? dayDiff(today, w) : null;
  if(diff === 0) return `today at ${time}`;
  if(diff === 1) return `tomorrow at ${time}`;
  const weekday = WEEKDAYS[new Date(Date.UTC(w.y, w.mo - 1, w.d)).getUTCDay()];
  return `${weekday} ${w.d} ${MONTHS[w.mo - 1]} at ${time}`;
}

export function sleepText(t, fetchedAt){
  return `off in ${durationText(remaining(t, fetchedAt))}`;
}

export function startText(t, fetchedAt){
  return `on ${startWhenText(t, fetchedAt)}`;
}

// ── the dialog ──────────────────────────────────────────────────────────────

function el(tag, className, text){
  const e = document.createElement(tag);
  if(className) e.className = className;
  if(text !== undefined) e.textContent = text;
  return e;
}

function presetLabel(m){
  if(m < 60) return `${m} min`;
  return m % 60 ? `${Math.floor(m / 60)} h ${m % 60}` : `${m / 60} h`;
}

// Resolves to one of
//   {kind: "sleep", minutes} | {kind: "start", days, at} | {kind: "cancel", id} | null
// The caller does the API work, so an error can land on the card like any
// other.
export function timerDialog({unitName, sleep, start, fetchedAt}){
  return new Promise((resolve) => {
    const overlay = el("div", "enroll-overlay");
    const card = el("div", "enroll-card timer-dialog");
    overlay.appendChild(card);
    card.appendChild(el("h2", "", `Timer — ${unitName}`));
    const onKey = (e) => { if(e.key === "Escape") close(null); };
    const close = (result) => {
      document.removeEventListener("keydown", onKey);
      overlay.remove();
      resolve(result);
    };
    document.addEventListener("keydown", onKey);

    // --- switch off --------------------------------------------------------
    const off = el("div", "timer-section");
    off.appendChild(el("h3", "", "Switch off"));
    if(sleep){
      const now = el("div", "timer-current");
      now.appendChild(el("span", "", `Switches ${sleepText(sleep, fetchedAt)}.`));
      const cancel = el("button", "enroll-btn secondary", "Cancel it");
      cancel.onclick = () => close({kind: "cancel", id: sleep.id});
      now.appendChild(cancel);
      off.appendChild(now);
    }
    const presets = el("div", "row");
    SLEEP_PRESETS.forEach((m) => {
      const b = el("button", "pill", presetLabel(m));
      b.onclick = () => close({kind: "sleep", minutes: m});
      presets.appendChild(b);
    });
    off.appendChild(presets);
    const custom = el("div", "timer-inline");
    const minutes = el("input", "enroll-input");
    minutes.type = "number";
    minutes.min = "1";
    minutes.max = String(MAX_SLEEP_MINUTES);
    minutes.placeholder = "minutes";
    const setSleep = el("button", "enroll-btn", "Set");
    const sleepErr = el("div", "enroll-error hidden");
    setSleep.onclick = () => {
      const m = Math.round(Number(minutes.value));
      if(!(m >= 1 && m <= MAX_SLEEP_MINUTES)){
        sleepErr.textContent = `From 1 to ${MAX_SLEEP_MINUTES} minutes.`;
        sleepErr.classList.remove("hidden");
        return;
      }
      close({kind: "sleep", minutes: m});
    };
    custom.append(minutes, setSleep);
    off.append(custom, sleepErr);
    card.appendChild(off);

    // --- switch on later ---------------------------------------------------
    const on = el("div", "timer-section");
    on.appendChild(el("h3", "", "Switch on later"));
    if(start){
      const now = el("div", "timer-current");
      now.appendChild(el("span", "", `Switches ${startText(start, fetchedAt)}.`));
      const cancel = el("button", "enroll-btn secondary", "Cancel it");
      cancel.onclick = () => close({kind: "cancel", id: start.id});
      now.appendChild(cancel);
      on.appendChild(now);
    }
    const when = el("div", "timer-inline");
    const days = el("select", "enroll-input");
    for(let d = 0; d <= MAX_START_DAYS; d++){
      const o = el("option", "", d === 0 ? "today" : d === 1 ? "tomorrow" : `in ${d} days`);
      o.value = String(d);
      days.appendChild(o);
    }
    // Opens on the pending start, if there is one, so changing only its time
    // does not quietly move its day; otherwise tomorrow at seven.
    days.value = "1";
    const time = el("input", "enroll-input");
    time.type = "time";
    time.value = "07:00";
    if(start){
      const w = wallClock(start.fires_at);
      const today = serverToday(start, fetchedAt);
      if(w) time.value = `${w.hh}:${w.mm}`;
      if(w && today){
        const d = dayDiff(today, w);
        if(d >= 0 && d <= MAX_START_DAYS) days.value = String(d);
      }
    }
    when.append(days, time);
    on.appendChild(when);
    const setStart = el("button", "enroll-btn", "Set");
    const startErr = el("div", "enroll-error hidden");
    setStart.onclick = () => {
      if(!/^\d{2}:\d{2}$/.test(time.value)){
        startErr.textContent = "Choose a time.";
        startErr.classList.remove("hidden");
        return;
      }
      close({kind: "start", days: Number(days.value), at: time.value});
    };
    on.append(setStart, startErr);
    on.appendChild(el("p", "enroll-hint",
      "It switches the unit on with the mode and temperature it last had. The time is the server's clock, which is the one schedules use too."));
    card.appendChild(on);

    const row = el("div", "btn-row");
    const done = el("button", "enroll-btn secondary", "Close");
    done.onclick = () => close(null);
    row.appendChild(done);
    card.appendChild(row);

    overlay.addEventListener("click", (e) => { if(e.target === overlay) close(null); });
    document.body.appendChild(overlay);
  });
}

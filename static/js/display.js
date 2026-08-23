// display.js — per-browser client preferences: temperature unit and beep.
//
// Neither changes the wire contract. Temperatures are always Celsius on the
// wire (16–30 in 0.5° steps) and converted here for presentation, like the
// mobile app's °C/°F toggle. Both choices live in localStorage, so they are per
// browser rather than server-side — two people can hold different preferences
// against the same Breeze Core.

const UNIT_KEY = "meow_ac_temp_unit";
const BEEP_KEY = "meow_ac_beep";

export function tempUnit(){
  return localStorage.getItem(UNIT_KEY) === "F" ? "F" : "C";
}
export function setTempUnit(u){
  localStorage.setItem(UNIT_KEY, u === "F" ? "F" : "C");
}
export function toggleTempUnit(){
  const next = tempUnit() === "C" ? "F" : "C";
  setTempUnit(next);
  return next;
}

// Should the unit chirp when it accepts a command? `beep` is an optional field
// on ControlRequest, and the server treats absent-or-false as silent — that is
// deliberate, so a 2am schedule or curve never wakes anybody. This is therefore
// an opt-IN for interactive use, defaulting to off to match both the server
// policy and the mobile app's ClimateSettings.beep.
export function beepEnabled(){
  return localStorage.getItem(BEEP_KEY) === "1";
}
export function setBeep(on){
  localStorage.setItem(BEEP_KEY, on ? "1" : "0");
}
export function toggleBeep(){
  const next = !beepEnabled();
  setBeep(next);
  return next;
}

// Format a Celsius value in the active unit. `showUnit` appends °C/°F;
// otherwise just a degree sign. Null/undefined -> "--°".
export function fmtTemp(celsius, { showUnit = true } = {}){
  if(celsius === null || celsius === undefined) return "--°";
  const f = tempUnit() === "F";
  const v = f ? (celsius * 9 / 5 + 32) : celsius;
  return v.toFixed(1) + "°" + (showUnit ? (f ? "F" : "C") : "");
}

// Explain, in words, a change the air conditioner refused.
//
// A unit that ignores part of a command still answers it, with its state
// unchanged, so from the panel a refused change just looked like a control
// springing back -- indistinguishable from a bug. Since 4.1.1 the server says
// which fields the unit did not take (`not_applied` in the control reply), and
// this turns that into a sentence that is clear about whose doing it was.
//
// The reasons are what was measured on real units, not guesses: none moves its
// flaps while switched off; while heating, most hold the flaps still until
// warm air is coming out, and ignore flap changes until then (in fan, dry,
// cool and auto the flaps change at once); and many offer eco only while
// cooling. Plain text only -- the caller sets textContent, never innerHTML.

const NAMES = {
  swing_mode: "the flap change",
  eco: "eco",
  turbo: "turbo",
  fan_speed: "the fan speed",
  target_temperature: "the temperature",
  operational_mode: "the mode",
  power_state: "the power change",
};

function joinNames(fields){
  const n = fields.map(f => NAMES[f] || f.replace(/_/g, " "));
  if(n.length <= 1) return n[0] || "the change";
  return n.slice(0, -1).join(", ") + " and " + n[n.length - 1];
}

function reasonFor(field, s){
  const heating = s.operational_mode === "HEAT";
  switch(field){
    case "swing_mode":
      if(!s.power_state) return "Units only move their flaps while they are running.";
      if(heating) return "While heating, it holds its flaps still until warm air is coming out, "
        + "which can take a few minutes after switching on or into heating. Try again shortly.";
      return "It may need a moment before it will take this. Try again shortly.";
    case "eco":
      return heating
        ? "Many units offer eco only while cooling, not while heating."
        : "Many units offer eco only in some modes.";
    case "turbo":
      return "Many units offer turbo only in some modes.";
    default:
      return "It does not take this in its current mode.";
  }
}

// The message for a control reply, or null when the unit took everything.
export function notAppliedMessage(s){
  const fields = Array.isArray(s && s.not_applied) ? s.not_applied : [];
  if(!fields.length) return null;
  const reasons = [];
  for(const f of fields){
    const r = reasonFor(f, s);
    if(!reasons.includes(r)) reasons.push(r);
  }
  return "The air conditioner didn't accept " + joinNames(fields)
    + " — the unit refused it, not Breeze Core. " + reasons.join(" ");
}

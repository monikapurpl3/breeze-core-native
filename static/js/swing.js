// swing.js — the two physical flaps <-> swing_mode enum mapping.
//
// The UI shows two independent toggles (vertical ↕, horizontal ↔) but
// the API speaks a single swing_mode enum. This computes the new enum
// value when one flap is flipped, leaving the other's state intact
// (flipping ↕ on while ↔ is already on yields BOTH, not VERTICAL).

export function nextSwingMode(current, axis){
  const v = current === "VERTICAL" || current === "BOTH";
  const h = current === "HORIZONTAL" || current === "BOTH";
  const newV = axis === "v" ? !v : v;
  const newH = axis === "h" ? !h : h;
  if(newV && newH) return "BOTH";
  if(newV) return "VERTICAL";
  if(newH) return "HORIZONTAL";
  return "OFF";
}

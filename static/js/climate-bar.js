// climate-bar.js — the indoor / outdoor / target bar.
//
// A port of the Android app's ClimateBar, rule for rule, so the two clients
// describe the same room the same way. One rule covers every case, which is
// worth stating because the obvious description of this widget is three rules
// that can contradict each other:
//
//   * the WARMEST of the three temperatures sets the top of the scale — the
//     bar's full width is that value;
//   * the other two are drawn from the floor as overlapping fills: the warmer
//     of them lighter, the cooler darker and painted on top.
//
// Both of the cases it was asked for fall out of that:
//
//   cooling — outdoor 33, indoor 27, target 24
//     the scale tops out at the outdoor reading; indoor is the lighter fill;
//     target is the darker fill over it.
//   heating — outdoor 5, indoor 20, target 23
//     the scale tops out at the target; indoor is again the lighter fill; the
//     outdoor reading is the darker one on top.
//
// …and so does "if indoor is below target, the lightnesses swap": with indoor 22
// and target 25 the warmer of the pair is now the target, so it takes the
// lighter shade. No special case needed — warmer is always lighter.
//
// The geometry is computed in Celsius regardless of the display unit; only the
// labels are converted, so a °F user gets identical proportions.

import { fmtTemp } from "./display.js";

// Readings outside this window are sensor nonsense, not weather. Units without
// an outdoor probe don't always report null — some report a sentinel, and a bar
// scaled to 255 °C would render as an empty sliver with no hint as to why.
export const SANE_MIN_C = -50;
export const SANE_MAX_C = 80;

// The smallest scale span we'll draw. Without it, three equal readings give
// max === floor and every fraction becomes a division by zero.
const MIN_SPAN = 1;

/// A temperature that is present, finite, and physically plausible.
export function sanitiseTemp(value){
  if(value === null || value === undefined) return null;
  if(typeof value !== "number" || !Number.isFinite(value)) return null;
  if(value < SANE_MIN_C || value > SANE_MAX_C) return null;
  return value;
}

// The geometry, worked out separately from any DOM so the awkward cases can be
// reasoned about — and checked — without a browser.
export function climateBarModel({ indoor, outdoor, target }){
  const values = [sanitiseTemp(indoor), sanitiseTemp(outdoor), sanitiseTemp(target)]
    .filter(v => v !== null);

  // One reading can't express a relationship, so there's nothing to draw.
  if(values.length < 2) return { floor: 0, max: 1, lighter: null, darker: null, usable: false };

  values.sort((a, b) => a - b);
  const top = values[values.length - 1];
  const rest = values.slice(0, values.length - 1);

  // Floor at 0 °C normally; drop below only if a reading demands it, because a
  // scale starting at 0 would clamp a −8 °C outdoor reading to nothing and look
  // like a broken sensor.
  let floor = 0;
  if(values[0] < floor) floor = Math.floor(values[0]);
  let max = top;
  if(max - floor < MIN_SPAN) max = floor + MIN_SPAN;

  // rest is ascending: the last is the warmer (lighter), the first the cooler.
  //
  // With only two readings there is no pair to distinguish, so the single fill
  // takes the strong colour rather than the pale one — a lone washed-out bar
  // looks like a rendering fault, and the light shade only earns its meaning
  // when something darker sits on top of it.
  return {
    floor,
    max,
    lighter: rest.length > 1 ? rest[rest.length - 1] : null,
    darker: rest[0],
    usable: true,
  };
}

// Where a temperature sits along the bar, 0..1.
export function fractionFor(model, celsius){
  const span = model.max - model.floor;
  if(span <= 0) return 0;                       // belt and braces
  const fraction = (celsius - model.floor) / span;
  return Math.min(1, Math.max(0, fraction));
}

// Build the element. Returns { el, update }.
export function buildClimateBar(){
  const el = document.createElement("div");
  el.className = "climate";

  const labels = document.createElement("div");
  labels.className = "climate-labels";
  const indoorLabel = document.createElement("span");
  indoorLabel.className = "climate-indoor";
  const outdoorLabel = document.createElement("span");
  outdoorLabel.className = "climate-outdoor";
  labels.append(indoorLabel, outdoorLabel);

  const track = document.createElement("div");
  track.className = "climate-track";
  // Order matters: the lighter fill is drawn first and the darker one over it.
  const lighter = document.createElement("div");
  lighter.className = "climate-fill lighter";
  const darker = document.createElement("div");
  darker.className = "climate-fill darker";
  track.append(lighter, darker);

  el.append(labels, track);

  function update(state){
    const indoor = sanitiseTemp(state?.indoor_temperature);
    const outdoor = sanitiseTemp(state?.outdoor_temperature);
    const target = sanitiseTemp(state?.target_temperature);

    // Labels always show what there is, even when the bar cannot be drawn: "I:
    // 27.0°" alone is still worth reading.
    indoorLabel.textContent = `I: ${fmtTemp(indoor, {showUnit: false})}`;
    outdoorLabel.textContent = outdoor === null
      ? ""                                        // no probe: say nothing
      : `O: ${fmtTemp(outdoor, {showUnit: false})}`;

    const model = climateBarModel({ indoor, outdoor, target });
    el.classList.toggle("unusable", !model.usable);
    if(!model.usable){
      lighter.style.width = "0%";
      darker.style.width = "0%";
      return;
    }
    // element.style, not a style attribute — the CSP forbids the latter.
    lighter.style.width = model.lighter === null
      ? "0%"
      : `${(fractionFor(model, model.lighter) * 100).toFixed(2)}%`;
    darker.style.width = `${(fractionFor(model, model.darker) * 100).toFixed(2)}%`;
  }

  return { el, update };
}

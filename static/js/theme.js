// theme.js — predefined Material You-like colour palettes for the web UI.
//
// A palette just sets a single seed colour via <html data-palette="…">;
// styles.css derives the accent and the hue-tinted neutral surfaces from it
// with color-mix, so the whole panel feels cohesive. The choice is per-browser
// (localStorage). Swatch colours are set with element.style in JS, which the
// strict CSP allows (unlike inline style="" attributes or <style> blocks).

const PALETTE_KEY = "meow_ac_palette";

// A theme derived from any master colour. Stored separately from the palette id
// so picking a preset does not destroy a colour somebody spent a minute
// choosing, and going back to "custom" restores it.
const CUSTOM_KEY = "meow_ac_custom_seed";
const CUSTOM_ID = "custom";

export const PALETTES = [
  { id: "teal",   name: "Teal",   seed: "#4fd1c5" },
  { id: "indigo", name: "Indigo", seed: "#7aa2f7" },
  { id: "violet", name: "Violet", seed: "#b48ead" },
  { id: "rose",   name: "Rose",   seed: "#ec7fa9" },
  { id: "green",  name: "Green",  seed: "#7bd88f" },
  { id: "amber",  name: "Amber",  seed: "#e0af68" },
];

export function currentPalette(){
  const id = localStorage.getItem(PALETTE_KEY);
  if(id === CUSTOM_ID && customSeed()) return CUSTOM_ID;
  return PALETTES.some(p => p.id === id) ? id : "teal";
}

export function applyPalette(id){
  const root = document.documentElement;
  if(id === CUSTOM_ID){
    const seed = customSeed();
    if(seed){
      // One property; styles.css color-mixes the accent and every neutral out
      // of it. element.style is CSP-safe where a style="" attribute is not.
      root.style.setProperty("--seed", seed);
      root.dataset.palette = CUSTOM_ID;
      localStorage.setItem(PALETTE_KEY, CUSTOM_ID);
      return;
    }
    // Asked for custom with no colour chosen: fall through to the default
    // rather than leaving the panel unstyled.
  }
  const p = PALETTES.find(x => x.id === id) || PALETTES[0];
  // Remove the override, or a preset would keep the custom hue underneath it.
  root.style.removeProperty("--seed");
  root.dataset.palette = p.id;
  localStorage.setItem(PALETTE_KEY, p.id);
}

export function initPalette(){ applyPalette(currentPalette()); }

// Build the header "Theme" button + swatch popover. Returns the wrapper node.
export function buildPalettePicker(){
  const wrap = document.createElement("div");
  wrap.className = "palette-picker";

  const btn = document.createElement("button");
  btn.className = "ghost-btn";
  btn.type = "button";
  btn.title = "Choose a colour theme";
  btn.textContent = "🎨 Theme";

  const pop = document.createElement("div");
  pop.className = "palette-pop hidden";

  const markActive = () => {
    const active = currentPalette();
    pop.querySelectorAll(".swatch").forEach(s =>
      s.classList.toggle("active", s.dataset.id === active));
  };

  PALETTES.forEach(p => {
    const s = document.createElement("button");
    s.className = "swatch";
    s.type = "button";
    s.dataset.id = p.id;
    s.title = p.name;
    s.setAttribute("aria-label", p.name);
    s.style.background = p.seed;   // CSP-safe (element.style, not an attribute)
    s.addEventListener("click", () => {
      applyPalette(p.id);
      markActive();
      pop.classList.add("hidden");
    });
    pop.appendChild(s);
  });

  // A master colour of the user's own, alongside the presets. Native
  // <input type="color"> rather than a hand-built wheel: it is one element, it
  // is accessible, and every platform already has a colour picker people know.
  const custom = document.createElement("label");
  custom.className = "swatch custom";
  custom.title = "Pick any colour";
  custom.dataset.id = CUSTOM_ID;

  const input = document.createElement("input");
  input.type = "color";
  input.value = customSeed() || PALETTES[0].seed;
  custom.style.background = input.value;

  // `input` fires continuously while dragging, which is the point: the whole
  // panel recolours live, so a colour is chosen by seeing it rather than by
  // guessing at a hex code.
  input.addEventListener("input", () => {
    if(setCustomSeed(input.value)){
      custom.style.background = input.value;
      applyPalette(CUSTOM_ID);
      markActive();
    }
  });
  custom.appendChild(input);
  pop.appendChild(custom);

  btn.addEventListener("click", (e) => {
    e.stopPropagation();
    pop.classList.toggle("hidden");
    markActive();
  });
  pop.addEventListener("click", (e) => e.stopPropagation());
  document.addEventListener("click", () => pop.classList.add("hidden"));

  wrap.appendChild(btn);
  wrap.appendChild(pop);
  return wrap;
}

// --- a theme from any master colour ----------------------------------------
//
// The six palettes above are seeds; so is this. styles.css derives the accent
// and every hue-tinted neutral from `--seed` with color-mix, so a custom theme
// sets exactly one property and the rest of the panel follows. That is why this
// is a few lines rather than a colour-science exercise.

/// A `#rrggbb` string, or null if there isn't one.
export function customSeed(){
  const value = localStorage.getItem(CUSTOM_KEY);
  return isHexColour(value) ? value.toLowerCase() : null;
}

export function setCustomSeed(hex){
  if(!isHexColour(hex)) return false;
  localStorage.setItem(CUSTOM_KEY, hex.toLowerCase());
  return true;
}

/// `#abc` and `#aabbcc`, with or without the hash, are all a person might type.
export function isHexColour(value){
  return typeof value === "string" && /^#?([0-9a-f]{3}|[0-9a-f]{6})$/i.test(value.trim());
}

/// Normalise to `#rrggbb`, which is what <input type="color"> wants.
export function normaliseHex(value){
  if(!isHexColour(value)) return null;
  let hex = value.trim().replace(/^#/, "").toLowerCase();
  if(hex.length === 3) hex = hex.split("").map(c => c + c).join("");
  return `#${hex}`;
}

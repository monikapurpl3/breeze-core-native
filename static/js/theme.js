// theme.js — predefined Material You-like colour palettes for the web UI.
//
// A palette just sets a single seed colour via <html data-palette="…">;
// styles.css derives the accent and the hue-tinted neutral surfaces from it
// with color-mix, so the whole panel feels cohesive. The choice is per-browser
// (localStorage). Swatch colours are set with element.style in JS, which the
// strict CSP allows (unlike inline style="" attributes or <style> blocks).

const PALETTE_KEY = "meow_ac_palette";

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
  return PALETTES.some(p => p.id === id) ? id : "teal";
}

export function applyPalette(id){
  const p = PALETTES.find(x => x.id === id) || PALETTES[0];
  document.documentElement.dataset.palette = p.id;
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

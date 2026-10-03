// a11y.js — what every dialog and every spoken update goes through, so that a
// keyboard, a screen reader, switch access and voice control get the same
// panel a mouse does.
//
// Two things, both used from several modules:
//
//   announce(text)        say something without moving focus, through one
//                         polite live region that is never made inert;
//   dialog(overlay, opts) make one of the panel's overlays a modal dialog:
//                         role and name, the page behind it inert, focus
//                         inside, Escape to close, and focus back on whatever
//                         opened it once it is gone.

let region = null;

// One region for the whole page, created on first use and kept outside
// everything dialog() makes inert -- an inert live region says nothing.
function liveRegion(){
  if(!region){
    region = document.createElement("div");
    region.id = "a11yLive";
    region.className = "sr-only";
    region.setAttribute("role", "status");
    region.setAttribute("aria-live", "polite");
    region.setAttribute("aria-atomic", "true");
    document.body.appendChild(region);
  }
  return region;
}

export function announce(text){
  const r = liveRegion();
  // Cleared first, and the text set a frame later: a screen reader reads a
  // region when it CHANGES, so the same message twice in a row would
  // otherwise be read once.
  r.textContent = "";
  requestAnimationFrame(() => { r.textContent = text; });
}

let serial = 0;

// overlay: the full-screen backdrop, whose first child is the dialog card.
// opts.onEscape: called on Escape; leave it out for a dialog that must not be
// dismissed (pairing). Returns release(), which undoes it all; it also runs by
// itself when the overlay is removed from the page, which is how most of the
// panel's dialogs close.
export function dialog(overlay, { onEscape } = {}){
  const card = overlay.firstElementChild || overlay;
  card.setAttribute("role", "dialog");
  card.setAttribute("aria-modal", "true");
  const heading = card.querySelector("h2, h3");
  if(heading){
    if(!heading.id) heading.id = "dialogTitle" + (++serial);
    card.setAttribute("aria-labelledby", heading.id);
  }

  const opener = document.activeElement;
  const live = liveRegion();
  // Everything else on the page, made unreachable while this is open: not
  // focusable, not clickable, not read. Only what this call changed is
  // restored, so a dialog opened from inside another one (a confirmation from
  // Programs) leaves the outer one inert until it closes itself.
  const others = Array.from(document.body.children)
    .filter(el => el !== overlay && el !== live && !el.inert);
  others.forEach(el => { el.inert = true; });

  const onKey = (e) => {
    if(e.key === "Escape" && onEscape){ e.stopPropagation(); onEscape(); }
  };
  overlay.addEventListener("keydown", onKey);

  let released = false;
  const release = () => {
    if(released) return;
    released = true;
    watcher.disconnect();
    overlay.removeEventListener("keydown", onKey);
    others.forEach(el => { el.inert = false; });
    if(opener && opener.isConnected && typeof opener.focus === "function") opener.focus();
  };
  const watcher = new MutationObserver(() => { if(!overlay.isConnected) release(); });
  watcher.observe(document.body, { childList: true });

  // Into the dialog: its first field, or else its first button that can be
  // pressed (Nerd's "Copy JSON" is disabled until the data arrives, and focus
  // on a disabled button silently goes nowhere), or else the card itself, so a
  // screen reader starts reading at the dialog and not at the page behind it.
  const first = card.querySelector("input:not([type=hidden]):not([disabled]), select:not([disabled]), textarea:not([disabled])")
             || card.querySelector("button:not([disabled]), [href], [tabindex]:not([tabindex='-1'])");
  if(first){
    first.focus();
  }else{
    card.tabIndex = -1;
    card.focus();
  }
  return release;
}

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

// The drag rectangle is reported in this window's own CSS pixels. Rust converts it to
// global physical coordinates using the overlay window's real position and scale
// factor, so mixed-DPI multi-monitor setups stay correct without the webview knowing
// anything about scaling.

const marquee = document.getElementById("marquee") as HTMLElement;
const readout = document.getElementById("readout") as HTMLElement;

let anchor: { x: number; y: number } | null = null;
let settled = false;

function cancel() {
  if (settled) return;
  settled = true;
  void invoke("crop_cancel");
}

function draw(cx: number, cy: number) {
  if (!anchor) return;
  const left = Math.min(anchor.x, cx);
  const top = Math.min(anchor.y, cy);
  const width = Math.abs(cx - anchor.x);
  const height = Math.abs(cy - anchor.y);

  marquee.hidden = false;
  marquee.style.left = `${left}px`;
  marquee.style.top = `${top}px`;
  marquee.style.width = `${width}px`;
  marquee.style.height = `${height}px`;
  marquee.dataset.flip = String(top + height + 26 > window.innerHeight);

  // Show the size the user will actually get, in device pixels.
  const dpr = window.devicePixelRatio || 1;
  readout.textContent = `${Math.round(width * dpr)} × ${Math.round(height * dpr)}`;
}

window.addEventListener("pointerdown", (e) => {
  if (settled) return;
  // Right/middle click cancels. Esc needs keyboard focus, which lives on whichever
  // overlay was opened or last clicked; this works on any monitor without it.
  if (e.button !== 0) {
    cancel();
    return;
  }
  anchor = { x: e.clientX, y: e.clientY };
  document.body.dataset.dragging = "true";
  draw(e.clientX, e.clientY);
});

window.addEventListener("contextmenu", (e) => e.preventDefault());

window.addEventListener("pointermove", (e) => {
  if (anchor) draw(e.clientX, e.clientY);
});

window.addEventListener("pointerup", (e) => {
  if (!anchor || settled) return;
  const start = anchor;
  anchor = null;
  document.body.dataset.dragging = "false";
  marquee.hidden = true;

  const x = Math.min(start.x, e.clientX);
  const y = Math.min(start.y, e.clientY);
  const width = Math.abs(e.clientX - start.x);
  const height = Math.abs(e.clientY - start.y);

  // A click without a drag is a cancel, not a 0×0 capture.
  if (width < 4 || height < 4) {
    cancel();
    return;
  }

  settled = true;
  void invoke("crop_commit", { x, y, width, height });
});

window.addEventListener("keydown", (e) => {
  if (e.key === "Escape") {
    e.preventDefault();
    cancel();
  }
});

// This window is built once and reused: Rust hides it between crops and sends
// "crop:open" when the next one starts. Rust captured the screen before showing
// anything and keeps the window hidden until it hears the frame is in, so the live
// screen never flashes through first. With no frame (or a failure) it crops live.
const frame = document.getElementById("frame") as HTMLImageElement;
let frameUrl: string | null = null;
/** Bumped per open and per dismiss, so a slow step from an earlier crop gives up. */
let token = 0;

function dropFrame() {
  frame.hidden = true;
  frame.removeAttribute("src");
  if (frameUrl) URL.revokeObjectURL(frameUrl);
  frameUrl = null;
}

async function prepare() {
  const mine = ++token;
  // The page can load (and run this) before any crop has been asked for.
  if (!(await invoke<boolean>("crop_active")) || mine !== token) return;

  settled = false;
  anchor = null;
  marquee.hidden = true;
  document.body.dataset.dragging = "false";
  void applyTheme();

  try {
    const png = await invoke<ArrayBuffer>("crop_frame");
    if (mine !== token) return;
    dropFrame();
    if (png && png.byteLength > 0) {
      frameUrl = URL.createObjectURL(new Blob([png], { type: "image/png" }));
      frame.src = frameUrl;
      // Wait for the load, not for decode(): a hidden window may hold decoding back
      // until it is shown, and waiting on that is what kept the overlay from appearing.
      await new Promise((done) => {
        frame.onload = done;
        frame.onerror = done;
        setTimeout(done, 300);
      });
      if (mine !== token) return;
      frame.hidden = false;
    }
  } catch {
    dropFrame();
  }
  if (mine === token) {
    void invoke("crop_ready");
    window.focus();
  }
}

// The overlay the user dragged on tells the others to stand down, so a stray
// pointerup elsewhere cannot fire a second capture. The frame is let go a moment later,
// once Rust has hidden the window, so the live screen never shows through it.
void listen("crop:dismiss", () => {
  settled = true;
  const mine = ++token;
  setTimeout(() => {
    if (mine === token) dropFrame();
  }, 250);
});

void listen("crop:open", () => void prepare());

async function applyTheme() {
  try {
    const theme = await invoke<string>("app_theme");
    if (theme === "light" || theme === "dark") document.documentElement.dataset.theme = theme;
    else delete document.documentElement.dataset.theme;
  } catch {
    // Keep the current theme.
  }
}

// The page may finish loading after the open event was sent; catch up if so.
void prepare();

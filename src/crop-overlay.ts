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

// The overlay the user dragged on tells the others to stand down, so a stray
// pointerup elsewhere cannot fire a second capture.
void listen("crop:dismiss", () => {
  settled = true;
});

// Rust captured the screen before this window existed, and keeps the window hidden
// until it is told the frame is painted, so the user never sees the live screen flash
// through first. With no frame (or a failure) the overlay simply crops live.
const frame = document.getElementById("frame") as HTMLImageElement;

async function showFrame() {
  try {
    const png = await invoke<ArrayBuffer>("crop_frame");
    if (png && png.byteLength > 0) {
      const url = URL.createObjectURL(new Blob([png], { type: "image/png" }));
      frame.src = url;
      // A hidden window may hold off decoding until it is shown; do not wait on that.
      await Promise.race([frame.decode(), new Promise((done) => setTimeout(done, 400))]);
      frame.hidden = false;
    }
  } catch {
    frame.hidden = true;
  }
  void invoke("crop_ready");
}

void showFrame();

void invoke<string>("app_theme")
  .then((theme) => {
    if (theme === "light" || theme === "dark") {
      document.documentElement.dataset.theme = theme;
    }
  })
  .catch(() => {});

window.focus();

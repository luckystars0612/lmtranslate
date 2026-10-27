import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { SpeechPlayer, type PlayerState } from "./speech-player";

/** Mirrors `windows::popup_window::PopupState` on the Rust side. */
interface PopupState {
  kind: "loading" | "result" | "error";
  targetLang: string;
  sourceKind: "text" | "image";
  original: string;
  translated: string;
  errorTitle: string;
  errorDetail: string;
  errorHint: string;
  canRetranslate: boolean;
  /** Short footer status, e.g. whether a rewrite replaced the selection. */
  note: string;
  /** True for errors the user can actually fix in Settings. */
  offerSettings: boolean;
  theme: "system" | "light" | "dark";
  languages: string[];
}

const el = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

const card = el<HTMLElement>("card");
const route = el<HTMLElement>("route");
const lang = el<HTMLSelectElement>("lang");
const gloss = el<HTMLElement>("gloss");
const originalRow = el<HTMLElement>("originalRow");
const original = el<HTMLParagraphElement>("original");
const translated = el<HTMLParagraphElement>("translated");
const err = el<HTMLElement>("err");
const errTitle = el<HTMLElement>("errTitle");
const errDetail = el<HTMLElement>("errDetail");
const errHint = el<HTMLElement>("errHint");
const foot = el<HTMLElement>("foot");
const stat = el<HTMLElement>("stat");
const copyBtn = el<HTMLButtonElement>("copy");
const settingsBtn = el<HTMLButtonElement>("openSettings");
const listenBtn = el<HTMLButtonElement>("listen");
const listenLabel = el<HTMLElement>("listenLabel");
const listenOriginalBtn = el<HTMLButtonElement>("listenOriginal");

let current: PopupState | null = null;
let copyResetTimer: number | undefined;
/** What the footer says when nothing is being read. */
let baseStat = "";
/** Which side is being read, if any. */
let reading: "translated" | "original" | null = null;

function applyTheme(theme: PopupState["theme"]) {
  if (theme === "system") delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = theme;
}

function fillLanguages(languages: string[], selected: string) {
  const wanted = [...languages];
  if (selected && !wanted.includes(selected)) wanted.unshift(selected);
  if (lang.dataset.filled === wanted.join("|")) {
    lang.value = selected;
    return;
  }
  lang.dataset.filled = wanted.join("|");
  lang.replaceChildren(
    ...wanted.map((name) => {
      const o = document.createElement("option");
      o.value = name;
      o.textContent = name;
      return o;
    }),
  );
  lang.value = selected;
}

function render(s: PopupState) {
  // A new translation (or a retranslation in progress) makes what is being read stale.
  if (reading && (s.kind !== "result" || s.translated !== current?.translated)) {
    player.stop();
  }
  current = s;
  applyTheme(s.theme);
  fillLanguages(s.languages, s.targetLang);
  lang.disabled = !s.canRetranslate;

  const from = s.sourceKind === "image" ? "SCREEN" : "SELECTION";
  route.innerHTML = `${from} <b>&rarr;</b> ${escapeHtml(s.targetLang.toUpperCase())}`;

  gloss.dataset.state = s.kind;

  if (s.kind === "error") {
    gloss.hidden = true;
    err.hidden = false;
    errTitle.textContent = s.errorTitle;
    errDetail.textContent = s.errorDetail;
    errHint.textContent = s.errorHint;
    errHint.hidden = !s.errorHint;

    // Errors the user can fix get a way to go fix them.
    copyBtn.hidden = true;
    settingsBtn.hidden = !s.offerSettings;
    stat.textContent = "";
    foot.hidden = !s.offerSettings;
  } else {
    err.hidden = true;
    gloss.hidden = false;
    copyBtn.hidden = false;
    settingsBtn.hidden = true;

    original.textContent = s.original;
    originalRow.hidden = s.original.length === 0;
    listenOriginalBtn.hidden = s.kind !== "result";

    if (s.kind === "loading") {
      translated.textContent =
        s.sourceKind === "image" ? "Reading the screen…" : "Translating…";
      foot.hidden = true;
    } else {
      translated.textContent = s.translated;
      foot.hidden = false;
      // A rewrite's outcome matters more than the character count.
      baseStat = s.note || `${[...s.translated].length} CHARS`;
      if (!reading) {
        stat.textContent = baseStat;
        stat.style.color = "";
      }
      resetCopyButton();
    }
  }

  requestAnimationFrame(fitWindow);
}

function escapeHtml(v: string) {
  return v.replace(/[&<>"]/g, (c) => `&#${c.charCodeAt(0)};`);
}

/** Ask Rust to snap the window to the card's rendered height (10px padding each side). */
function fitWindow() {
  const height = Math.ceil(card.getBoundingClientRect().height) + 20;
  void invoke("resize_popup", { height });
}

function resetCopyButton() {
  window.clearTimeout(copyResetTimer);
  copyBtn.textContent = "Copy translation";
  copyBtn.disabled = false;
}

copyBtn.addEventListener("click", async () => {
  if (!current) return;
  try {
    await invoke("copy_text", { text: current.translated });
    copyBtn.textContent = "Copied";
    copyBtn.disabled = true;
    copyResetTimer = window.setTimeout(resetCopyButton, 1400);
  } catch (e) {
    copyBtn.textContent = "Copy failed";
    copyResetTimer = window.setTimeout(resetCopyButton, 1400);
    console.error(e);
  }
});

// ── Read aloud ─────────────────────────────────────────────────────────────

const player = new SpeechPlayer((state: PlayerState) => {
  const busy = state.phase === "loading" || state.phase === "playing";
  if (!busy) reading = null;

  listenBtn.dataset.reading = String(busy && reading === "translated");
  listenOriginalBtn.dataset.reading = String(busy && reading === "original");
  listenLabel.textContent = busy && reading === "translated" ? "Stop" : "Listen";

  switch (state.phase) {
    case "idle":
      stat.textContent = baseStat;
      stat.style.color = "";
      break;
    case "loading":
      stat.textContent = "PREPARING VOICE…";
      break;
    case "playing":
      stat.textContent = state.fallback
        ? "READING OFFLINE — CLOUD VOICE FAILED"
        : `READING · ${state.engine.toUpperCase()}`;
      break;
    case "error":
      stat.textContent = state.message;
      stat.style.color = "var(--danger)";
      requestAnimationFrame(fitWindow);
      break;
  }
});

function readAloud(which: "translated" | "original") {
  if (!current || current.kind !== "result") return;
  if (reading === which && player.active) {
    player.stop();
    return;
  }
  stat.style.color = "";
  reading = which;
  // The translation's language is known; the original's is left to the voice.
  const text = which === "translated" ? current.translated : current.original;
  const lang = which === "translated" ? current.targetLang : null;
  void player.play(text, lang);
}

listenBtn.addEventListener("click", () => readAloud("translated"));
listenOriginalBtn.addEventListener("click", () => readAloud("original"));

el<HTMLButtonElement>("close").addEventListener("click", () => void invoke("close_popup"));

lang.addEventListener("change", () => {
  void invoke("retranslate", { targetLang: lang.value });
});

window.addEventListener("keydown", (e) => {
  if (e.key === "Escape") {
    e.preventDefault();
    void invoke("close_popup");
  }
});

// A deliberate interaction cancels the auto-close timer. Pointer *movement* is
// deliberately not in this list: the popup opens under the cursor, so tracking moves
// would cancel the timer every time.
for (const ev of ["pointerdown", "keydown", "wheel"] as const) {
  window.addEventListener(ev, () => void invoke("keep_popup_open"), { passive: true });
}

settingsBtn.addEventListener("click", async () => {
  await invoke("open_settings");
  void invoke("close_popup");
});

// Register the listener *before* pulling the current state. The other order leaves a
// gap where a fast-failing flow (a missing API key, say) emits its error between the
// pull and the subscription, stranding the popup on "Translating…" forever.
async function boot() {
  await listen<PopupState>("popup:state", (e) => render(e.payload));
  render(await invoke<PopupState>("get_popup_state"));
}

void boot().catch(console.error);

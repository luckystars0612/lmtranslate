import { invoke } from "@tauri-apps/api/core";

interface ProviderMeta {
  id: string;
  label: string;
  defaultBaseUrl: string;
  defaultModel: string;
  supportsImage: boolean;
}

interface ProviderConfig {
  apiKey: string;
  baseUrl: string;
  model: string;
}

interface Settings {
  provider: string;
  providers: Record<string, ProviderConfig>;
  targetLang: string;
  hotkeyText: string;
  hotkeyScreenshot: string;
  launchOnLogin: boolean;
  popupAutoCloseMs: number;
  theme: "system" | "light" | "dark";
}

interface PlatformReport {
  os: string;
  session: string;
  backend: string;
  supportsTextSelection: boolean;
  supportsScreenCapture: boolean;
  notes: string[];
  needsAccessibilityPrompt: boolean;
  configPath: string;
  logPath: string;
}

interface Check {
  tone: "ok" | "warn" | "bad" | "busy";
  message: string;
}

const el = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

const providerSel = el<HTMLSelectElement>("provider");
const baseUrl = el<HTMLInputElement>("baseUrl");
const model = el<HTMLInputElement>("model");
const apiKey = el<HTMLInputElement>("apiKey");
const revealKey = el<HTMLButtonElement>("revealKey");
const testConn = el<HTMLButtonElement>("testConn");
const connStatus = el<HTMLParagraphElement>("connStatus");
const targetLang = el<HTMLInputElement>("targetLang");
const langList = el<HTMLDataListElement>("langList");
const hkText = el<HTMLButtonElement>("hkText");
const hkShot = el<HTMLButtonElement>("hkShot");
const autoClose = el<HTMLSelectElement>("autoClose");
const themeSel = el<HTMLSelectElement>("theme");
const launchOnLogin = el<HTMLInputElement>("launchOnLogin");
const platform = el<HTMLParagraphElement>("platform");
const platformNotes = el<HTMLUListElement>("platformNotes");
const logView = el<HTMLPreElement>("log");
const saveState = el<HTMLElement>("saveState");

let settings: Settings;
let providers: ProviderMeta[] = [];

function setStatus(node: HTMLElement, check: Check | null) {
  if (!check) {
    node.hidden = true;
    return;
  }
  node.hidden = false;
  node.dataset.tone = check.tone;
  node.textContent = check.message;
}

function applyTheme(theme: Settings["theme"]) {
  if (theme === "system") delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = theme;
}

function currentProviderConfig(): ProviderConfig {
  const meta = providers.find((p) => p.id === settings.provider);
  return (
    settings.providers[settings.provider] ?? {
      apiKey: "",
      baseUrl: meta?.defaultBaseUrl ?? "",
      model: meta?.defaultModel ?? "",
    }
  );
}

function renderProviderFields() {
  const cfg = currentProviderConfig();
  const meta = providers.find((p) => p.id === settings.provider);
  baseUrl.value = cfg.baseUrl;
  baseUrl.placeholder = meta?.defaultBaseUrl ?? "";
  model.value = cfg.model;
  model.placeholder = meta?.defaultModel ?? "";
  apiKey.value = cfg.apiKey;
  setStatus(connStatus, null);
}

function renderForm() {
  providerSel.value = settings.provider;
  renderProviderFields();
  targetLang.value = settings.targetLang;
  hkText.textContent = settings.hotkeyText || "Click to set";
  hkShot.textContent = settings.hotkeyScreenshot || "Click to set";
  autoClose.value = String(settings.popupAutoCloseMs);
  themeSel.value = settings.theme;
  launchOnLogin.checked = settings.launchOnLogin;
  applyTheme(settings.theme);
}

/** Pull the form into `settings` and persist. Rust re-registers hotkeys, rebuilds the
 *  provider and applies the launch-on-login change as part of the save. */
async function save() {
  settings.providers[settings.provider] = {
    apiKey: apiKey.value,
    baseUrl: baseUrl.value.trim(),
    model: model.value.trim(),
  };
  settings.targetLang = targetLang.value.trim() || settings.targetLang;
  settings.popupAutoCloseMs = Number(autoClose.value);
  settings.theme = themeSel.value as Settings["theme"];
  settings.launchOnLogin = launchOnLogin.checked;
  applyTheme(settings.theme);

  try {
    settings = await invoke<Settings>("save_settings", { incoming: settings });
    const t = new Date().toLocaleTimeString();
    saveState.textContent = `SAVED ${t}`;
    saveState.style.color = "";
  } catch (e) {
    saveState.textContent = `NOT SAVED — ${String(e)}`;
    saveState.style.color = "var(--danger)";
  }
}

// ── Hotkey recording ───────────────────────────────────────────────────────

const PUNCT: Record<string, string> = {
  Comma: ",",
  Period: ".",
  Slash: "/",
  Backslash: "\\",
  Semicolon: ";",
  Quote: "'",
  BracketLeft: "[",
  BracketRight: "]",
  Minus: "-",
  Equal: "=",
  Backquote: "`",
  Space: "Space",
};

/** Build a Tauri accelerator string from a keydown, or null if it isn't a usable combo. */
function accelFrom(e: KeyboardEvent): string | null {
  if (["Control", "Alt", "Shift", "Meta"].includes(e.key)) return null;

  const mods: string[] = [];
  if (e.ctrlKey) mods.push("Control");
  if (e.altKey) mods.push("Alt");
  if (e.shiftKey) mods.push("Shift");
  if (e.metaKey) mods.push("Super");
  if (mods.length === 0) return null;

  const code = e.code;
  let key: string;
  if (/^Key[A-Z]$/.test(code)) key = code.slice(3);
  else if (/^Digit[0-9]$/.test(code)) key = code.slice(5);
  else if (/^F([1-9]|1[0-9]|2[0-4])$/.test(code)) key = code;
  else if (code in PUNCT) key = PUNCT[code];
  else if (["Enter", "Tab", "Backspace", "Delete", "Insert", "Home", "End"].includes(code))
    key = code;
  else return null;

  return [...mods, key].join("+");
}

function wireHotkey(
  button: HTMLButtonElement,
  status: HTMLElement,
  slot: "text" | "screenshot",
) {
  let recording = false;

  const stop = () => {
    recording = false;
    button.dataset.recording = "false";
    const value = slot === "text" ? settings.hotkeyText : settings.hotkeyScreenshot;
    button.textContent = value || "Click to set";
    // Put the real hotkeys back.
    void invoke("end_hotkey_capture");
  };

  button.addEventListener("click", async () => {
    // The OS delivers a registered global shortcut to whoever owns it, not to this
    // webview — so while recording, LM Translate lets go of its own hotkeys.
    // Otherwise pressing Alt+D here would fire a translation instead of being caught.
    await invoke("begin_hotkey_capture");
    recording = true;
    button.dataset.recording = "true";
    button.textContent = "Press a combination…  (Esc to cancel)";
    setStatus(status, null);
    button.focus();
  });

  button.addEventListener("blur", () => {
    if (recording) stop();
  });

  button.addEventListener("keydown", async (e) => {
    if (!recording) return;
    e.preventDefault();
    e.stopPropagation();

    if (e.key === "Escape") {
      stop();
      return;
    }

    const accel = accelFrom(e);
    if (!accel) {
      setStatus(status, {
        tone: "warn",
        message: "Needs at least one modifier (Ctrl, Alt, Shift or Super) plus a key.",
      });
      return;
    }

    button.textContent = accel;
    setStatus(status, { tone: "busy", message: "Checking availability…" });

    const check = await invoke<Check>("check_hotkey", { accel, slot });
    setStatus(status, check);

    if (check.tone !== "bad") {
      if (slot === "text") settings.hotkeyText = accel;
      else settings.hotkeyScreenshot = accel;
      await save();
    }
    stop();
  });
}

// ── Wiring ─────────────────────────────────────────────────────────────────

providerSel.addEventListener("change", async () => {
  settings.provider = providerSel.value;
  renderProviderFields();
  await save();
});

for (const node of [baseUrl, model, apiKey, targetLang]) {
  node.addEventListener("change", () => void save());
}
for (const node of [autoClose, themeSel, launchOnLogin]) {
  node.addEventListener("change", () => void save());
}

revealKey.addEventListener("click", () => {
  const shown = apiKey.type === "text";
  apiKey.type = shown ? "password" : "text";
  revealKey.textContent = shown ? "Show" : "Hide";
  revealKey.setAttribute("aria-pressed", String(!shown));
});

testConn.addEventListener("click", async () => {
  testConn.disabled = true;
  setStatus(connStatus, { tone: "busy", message: "Contacting the endpoint…" });
  try {
    const result = await invoke<Check>("test_connection", {
      provider: settings.provider,
      apiKey: apiKey.value,
      baseUrl: baseUrl.value.trim(),
      model: model.value.trim(),
    });
    setStatus(connStatus, result);
  } catch (e) {
    setStatus(connStatus, { tone: "bad", message: String(e) });
  } finally {
    testConn.disabled = false;
  }
});

el<HTMLButtonElement>("refreshLog").addEventListener("click", () => void loadLog());
el<HTMLButtonElement>("clearLog").addEventListener("click", async () => {
  await invoke("clear_log");
  await loadLog();
});
el<HTMLButtonElement>("openLogDir").addEventListener("click", () => void invoke("reveal_log"));
el<HTMLButtonElement>("openA11y").addEventListener("click", () =>
  invoke("open_accessibility_settings"),
);

async function loadLog() {
  const text = await invoke<string>("get_log");
  logView.textContent = text || "The log is empty.";
  logView.scrollTop = logView.scrollHeight;
}

async function boot() {
  [settings, providers] = await Promise.all([
    invoke<Settings>("get_settings"),
    invoke<ProviderMeta[]>("list_providers"),
  ]);

  providerSel.replaceChildren(
    ...providers.map((p) => {
      const o = document.createElement("option");
      o.value = p.id;
      o.textContent = p.label;
      return o;
    }),
  );

  const languages = await invoke<string[]>("list_languages");
  langList.replaceChildren(
    ...languages.map((name) => {
      const o = document.createElement("option");
      o.value = name;
      return o;
    }),
  );

  renderForm();
  wireHotkey(hkText, el<HTMLElement>("hkTextStatus"), "text");
  wireHotkey(hkShot, el<HTMLElement>("hkShotStatus"), "screenshot");

  const report = await invoke<PlatformReport>("platform_report");
  platform.textContent = `${report.backend}  ·  selection ${
    report.supportsTextSelection ? "supported" : "unavailable"
  }  ·  capture ${report.supportsScreenCapture ? "supported" : "unavailable"}`;
  platformNotes.replaceChildren(
    ...[...report.notes, `Settings file: ${report.configPath}`, `Log file: ${report.logPath}`].map(
      (n) => {
        const li = document.createElement("li");
        li.textContent = n;
        return li;
      },
    ),
  );
  el<HTMLElement>("a11yRow").hidden = !report.needsAccessibilityPrompt;

  await loadLog();
  saveState.textContent = "UP TO DATE";
}

void boot().catch((e) => {
  saveState.textContent = `FAILED TO LOAD — ${String(e)}`;
  saveState.style.color = "var(--danger)";
});

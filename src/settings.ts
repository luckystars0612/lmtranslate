import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { SpeechPlayer, type PlayerState } from "./speech-player";

interface ProviderMeta {
  id: string;
  label: string;
  defaultBaseUrl: string;
  defaultModel: string;
  /** Empty: the model list is fetched from the endpoint. */
  models: string[];
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
  replaceLang: string;
  hotkeyText: string;
  hotkeyReplace: string;
  hotkeyScreenshot: string;
  launchOnLogin: boolean;
  rewriteShowPopup: boolean;
  selectionBubble: boolean;
  speechProvider: "minimax" | "azure" | "local";
  speechMinimaxModel: string;
  speechMinimaxKey: string;
  azureSpeechKey: string;
  azureSpeechRegion: string;
  speechFallback: boolean;
  /** Keyed "engine:Language"; a missing entry means the engine's default voice. */
  speechVoices: Record<string, string>;
  popupAutoCloseMs: number;
  theme: "system" | "light" | "dark";
}

interface PlatformReport {
  os: string;
  session: string;
  backend: string;
  supportsTextSelection: boolean;
  supportsScreenCapture: boolean;
  supportsReplace: boolean;
  supportsSelectionBubble: boolean;
  notes: string[];
  needsAccessibilityPrompt: boolean;
  configPath: string;
  logPath: string;
  keyStorage: string;
}

/** Mirrors `speech::local::OfflineStatus`. */
interface OfflineStatus {
  supported: boolean;
  installed: boolean;
  installing: boolean;
  stage: "" | "runtime" | "model" | "unpacking";
  received: number;
  total: number;
  downloadMb: number;
  error: string;
}

/** Mirrors `speech::VoiceList`. */
interface VoiceList {
  defaultId: string;
  voices: { id: string; label: string }[];
}

interface Check {
  tone: "ok" | "warn" | "bad" | "busy";
  message: string;
}

const el = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

const providerSel = el<HTMLSelectElement>("provider");
const baseUrl = el<HTMLInputElement>("baseUrl");
const model = el<HTMLInputElement>("model");
const modelSelect = el<HTMLSelectElement>("modelSelect");
const refreshModels = el<HTMLButtonElement>("refreshModels");
const modelStatus = el<HTMLParagraphElement>("modelStatus");
const apiKey = el<HTMLInputElement>("apiKey");
const revealKey = el<HTMLButtonElement>("revealKey");
const testConn = el<HTMLButtonElement>("testConn");
const connStatus = el<HTMLParagraphElement>("connStatus");
const targetLang = el<HTMLInputElement>("targetLang");
const replaceLang = el<HTMLInputElement>("replaceLang");
const langList = el<HTMLDataListElement>("langList");
const hkText = el<HTMLButtonElement>("hkText");
const hkReplace = el<HTMLButtonElement>("hkReplace");
const hkShot = el<HTMLButtonElement>("hkShot");
const autoClose = el<HTMLSelectElement>("autoClose");
const rewriteShowPopup = el<HTMLInputElement>("rewriteShowPopup");
const selectionBubble = el<HTMLInputElement>("selectionBubble");
const speechProvider = el<HTMLSelectElement>("speechProvider");
const speechModel = el<HTMLSelectElement>("speechModel");
const speechMinimaxKey = el<HTMLInputElement>("speechMinimaxKey");
const azureKey = el<HTMLInputElement>("azureKey");
const azureRegion = el<HTMLSelectElement>("azureRegion");
const speechFallback = el<HTMLInputElement>("speechFallback");
const voiceLang = el<HTMLSelectElement>("voiceLang");
const voiceSelect = el<HTMLSelectElement>("voiceSelect");
const testVoice = el<HTMLButtonElement>("testVoice");
const voiceStatus = el<HTMLParagraphElement>("voiceStatus");
const offlineAction = el<HTMLButtonElement>("offlineAction");
const offlineStatus = el<HTMLParagraphElement>("offlineStatus");
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
  model.placeholder = meta?.defaultModel || "Model name";
  apiKey.value = cfg.apiKey;
  setStatus(connStatus, null);
  void loadModels();
}

function option(value: string, label: string) {
  const o = document.createElement("option");
  o.value = value;
  o.textContent = label;
  return o;
}

// ── Model dropdown ─────────────────────────────────────────────────────────

/** The dropdown entry that reveals a text box for a model the list does not have. */
const CUSTOM_MODEL = "__custom__";

/** `model` (the text box) stays the source of truth; the dropdown drives it. */
function fillModels(models: string[]) {
  const current = model.value.trim();
  const names = [...models];
  // A saved model the list does not know about stays selectable.
  if (current && !names.includes(current)) names.unshift(current);
  modelSelect.replaceChildren(...names.map((m) => option(m, m)), option(CUSTOM_MODEL, "Other model…"));
  modelSelect.value = current || CUSTOM_MODEL;
  model.hidden = modelSelect.value !== CUSTOM_MODEL;
}

/** MiniMax has a fixed list; the generic endpoints are asked what they serve. */
async function loadModels() {
  const meta = providers.find((p) => p.id === settings.provider);
  const fixed = meta?.models ?? [];
  refreshModels.hidden = fixed.length > 0;
  setStatus(modelStatus, null);
  if (fixed.length > 0) {
    fillModels(fixed);
    return;
  }
  fillModels([]);
  setStatus(modelStatus, { tone: "busy", message: "Asking the endpoint for its models…" });
  try {
    const models = await invoke<string[]>("list_models", {
      provider: settings.provider,
      baseUrl: baseUrl.value.trim(),
      apiKey: apiKey.value,
    });
    fillModels(models);
    setStatus(modelStatus, null);
  } catch (e) {
    setStatus(modelStatus, { tone: "warn", message: String(e) });
  }
}

modelSelect.addEventListener("change", () => {
  if (modelSelect.value === CUSTOM_MODEL) {
    model.hidden = false;
    model.focus();
    return;
  }
  model.hidden = true;
  model.value = modelSelect.value;
  void save();
});

refreshModels.addEventListener("click", () => void loadModels());

function renderForm() {
  providerSel.value = settings.provider;
  renderProviderFields();
  targetLang.value = settings.targetLang;
  replaceLang.value = settings.replaceLang;
  hkText.textContent = settings.hotkeyText || "Click to set";
  hkReplace.textContent = settings.hotkeyReplace || "Click to set";
  hkShot.textContent = settings.hotkeyScreenshot || "Click to set";
  autoClose.value = String(settings.popupAutoCloseMs);
  rewriteShowPopup.checked = settings.rewriteShowPopup;
  selectionBubble.checked = settings.selectionBubble;
  speechProvider.value = settings.speechProvider;
  speechModel.value = settings.speechMinimaxModel;
  speechMinimaxKey.value = settings.speechMinimaxKey;
  azureKey.value = settings.azureSpeechKey;
  if (settings.azureSpeechRegion && ![...azureRegion.options].some((o) => o.value === settings.azureSpeechRegion)) {
    azureRegion.append(option(settings.azureSpeechRegion, settings.azureSpeechRegion));
  }
  azureRegion.value = settings.azureSpeechRegion;
  speechFallback.checked = settings.speechFallback;
  showVoiceRows();
  themeSel.value = settings.theme;
  launchOnLogin.checked = settings.launchOnLogin;
  applyTheme(settings.theme);
}

/** The save most recently started, so an action that depends on it can wait for it. */
let lastSave: Promise<void> = Promise.resolve();

function save(): Promise<void> {
  lastSave = persist();
  return lastSave;
}

/** Pull the form into `settings` and persist. Rust re-registers hotkeys, rebuilds the
 *  provider and applies the launch-on-login change as part of the save. */
async function persist() {
  settings.providers[settings.provider] = {
    apiKey: apiKey.value,
    baseUrl: baseUrl.value.trim(),
    model: model.value.trim(),
  };
  settings.targetLang = targetLang.value.trim() || settings.targetLang;
  settings.replaceLang = replaceLang.value.trim() || settings.replaceLang;
  settings.popupAutoCloseMs = Number(autoClose.value);
  settings.rewriteShowPopup = rewriteShowPopup.checked;
  settings.selectionBubble = selectionBubble.checked;
  settings.speechProvider = speechProvider.value as Settings["speechProvider"];
  settings.speechMinimaxModel = speechModel.value;
  settings.speechMinimaxKey = speechMinimaxKey.value.trim();
  settings.azureSpeechKey = azureKey.value.trim();
  settings.azureSpeechRegion = azureRegion.value.trim();
  settings.speechFallback = speechFallback.checked;
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

/** Matches `hotkeys::Slot::parse` on the Rust side. */
type Slot = "text" | "replace" | "screenshot";

function hotkeyOf(slot: Slot): string {
  if (slot === "text") return settings.hotkeyText;
  if (slot === "replace") return settings.hotkeyReplace;
  return settings.hotkeyScreenshot;
}

function setHotkey(slot: Slot, accel: string) {
  if (slot === "text") settings.hotkeyText = accel;
  else if (slot === "replace") settings.hotkeyReplace = accel;
  else settings.hotkeyScreenshot = accel;
}

function wireHotkey(button: HTMLButtonElement, status: HTMLElement, slot: Slot) {
  let recording = false;

  const stop = () => {
    recording = false;
    button.dataset.recording = "false";
    button.textContent = hotkeyOf(slot) || "Click to set";
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
      setHotkey(slot, accel);
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

for (const node of [model, targetLang, replaceLang]) {
  node.addEventListener("change", () => void save());
}
// A generic endpoint's model list depends on where it is and who is asking.
for (const node of [baseUrl, apiKey]) {
  node.addEventListener("change", () => {
    void save();
    void loadModels();
  });
}
// The voice list comes from the account and region these point at.
for (const node of [speechMinimaxKey, azureKey, azureRegion]) {
  node.addEventListener("change", () => void save().then(loadVoices));
}
for (const node of [
  autoClose,
  rewriteShowPopup,
  selectionBubble,
  themeSel,
  launchOnLogin,
  speechModel,
  speechFallback,
]) {
  node.addEventListener("change", () => void save());
}

// ── Voice ──────────────────────────────────────────────────────────────────

function showVoiceRows() {
  for (const row of document.querySelectorAll<HTMLElement>("[data-voice]")) {
    row.hidden = row.dataset.voice !== speechProvider.value;
  }
}

speechProvider.addEventListener("change", () => {
  showVoiceRows();
  void save().then(loadVoices);
});

/** One sentence per language, so a preview sounds like what the popup will read. */
const SAMPLES: Record<string, string> = {
  Vietnamese: "Xin chào! Đây là giọng đọc bản dịch của bạn.",
  English: "Hello! This is how your translations will sound.",
  "Simplified Chinese": "你好！这是翻译朗读的声音。",
  "Traditional Chinese": "你好！這是翻譯朗讀的聲音。",
  Japanese: "こんにちは。翻訳はこの声で読み上げられます。",
  Korean: "안녕하세요. 번역문은 이 목소리로 읽어 드립니다.",
  Thai: "สวัสดีครับ นี่คือเสียงที่จะอ่านคำแปลของคุณ",
  French: "Bonjour ! Voici la voix qui lira vos traductions.",
  German: "Hallo! So klingen Ihre Übersetzungen.",
  Spanish: "¡Hola! Así sonarán tus traducciones.",
  Portuguese: "Olá! É assim que as suas traduções vão soar.",
  Italian: "Ciao! Ecco come suoneranno le tue traduzioni.",
  Russian: "Здравствуйте! Так будут звучать ваши переводы.",
  Arabic: "مرحبًا! هكذا ستبدو ترجماتك.",
  Hindi: "नमस्ते! आपके अनुवाद इस आवाज़ में पढ़े जाएँगे।",
  Indonesian: "Halo! Beginilah terjemahan Anda akan terdengar.",
};

/** The voice used for the current engine and language when none is picked. */
let defaultVoice = "";

function voiceKey() {
  return `${speechProvider.value}:${voiceLang.value}`;
}

async function loadVoices() {
  const engine = speechProvider.value;
  const lang = voiceLang.value;
  voiceSelect.disabled = true;
  voiceSelect.replaceChildren(option("", "Loading voices…"));
  try {
    const list = await invoke<VoiceList>("list_voices", { engine, lang });
    // The engine or language changed while this was loading; a newer call owns the box.
    if (engine !== speechProvider.value || lang !== voiceLang.value) return;
    defaultVoice = list.defaultId;
    if (engine === "local" && list.voices.length === 0) {
      voiceSelect.replaceChildren(option("", "The offline voice reads only English and Chinese"));
    } else {
      const auto = list.voices.find((v) => v.id === list.defaultId)?.label ?? list.defaultId;
      voiceSelect.replaceChildren(
        option("", `Automatic — ${auto}`),
        ...list.voices.map((v) => option(v.id, v.label)),
      );
    }
    const picked = settings.speechVoices[voiceKey()] ?? "";
    if (picked && ![...voiceSelect.options].some((o) => o.value === picked)) {
      voiceSelect.append(option(picked, picked));
    }
    voiceSelect.value = picked;
    setStatus(voiceStatus, null);
  } catch (e) {
    defaultVoice = "";
    voiceSelect.replaceChildren(option("", "Automatic"));
    setStatus(voiceStatus, { tone: "warn", message: String(e) });
  } finally {
    voiceSelect.disabled = false;
  }
}

voiceLang.addEventListener("change", () => void loadVoices());

voiceSelect.addEventListener("change", () => {
  if (voiceSelect.value) settings.speechVoices[voiceKey()] = voiceSelect.value;
  else delete settings.speechVoices[voiceKey()];
  void save();
});

const voicePlayer = new SpeechPlayer((state: PlayerState) => {
  testVoice.textContent = state.phase === "idle" || state.phase === "error" ? "Preview" : "Stop";
  switch (state.phase) {
    case "idle":
      setStatus(voiceStatus, null);
      break;
    case "loading":
      setStatus(voiceStatus, { tone: "busy", message: state.engine ? `Asking ${state.engine}…` : "Preparing…" });
      break;
    case "playing":
      setStatus(voiceStatus, {
        tone: state.fallback ? "warn" : "ok",
        message: state.fallback
          ? `Playing with the ${state.engine.toLowerCase()} — the cloud voice failed (see the debug log).`
          : `Playing with ${state.engine}.`,
      });
      break;
    case "error":
      setStatus(voiceStatus, { tone: "bad", message: state.message });
      break;
  }
});

testVoice.addEventListener("click", async () => {
  if (voicePlayer.active && testVoice.textContent === "Stop") {
    voicePlayer.stop();
    return;
  }
  // A key typed a moment ago is saved by the change event that clicking here fired.
  // Wait for that save rather than starting another: every save also clears the
  // translation cache.
  await lastSave;
  const lang = voiceLang.value;
  await voicePlayer.play(SAMPLES[lang] ?? SAMPLES.English, lang, {
    engine: speechProvider.value,
    voice: voiceSelect.value || defaultVoice,
  });
});

function renderOffline(status: OfflineStatus) {
  offlineAction.hidden = !status.supported;
  offlineAction.disabled = status.installing;

  if (!status.supported) {
    setStatus(offlineStatus, { tone: "warn", message: "Not available for this platform." });
    return;
  }
  if (status.installing) {
    const mb = (n: number) => Math.round(n / 1_000_000);
    const what = status.stage === "runtime" ? "runtime" : "voice model";
    const message =
      status.stage === "unpacking"
        ? "Unpacking…"
        : status.total > 0
          ? `Downloading the ${what}… ${mb(status.received)} / ${mb(status.total)} MB`
          : `Downloading the ${what}…`;
    offlineAction.textContent = "Downloading…";
    setStatus(offlineStatus, { tone: "busy", message });
    return;
  }

  offlineAction.textContent = status.installed
    ? "Remove"
    : `Download (about ${status.downloadMb} MB)`;
  if (status.error) {
    setStatus(offlineStatus, { tone: "bad", message: status.error });
  } else if (status.installed) {
    setStatus(offlineStatus, { tone: "ok", message: "Installed — ready to use without a connection." });
  } else {
    setStatus(offlineStatus, { tone: "busy", message: "Not downloaded." });
  }
}

offlineAction.addEventListener("click", async () => {
  const status = await invoke<OfflineStatus>("speech_offline_status");
  try {
    if (status.installed) await invoke("speech_offline_remove");
    else await invoke("speech_offline_install");
  } catch (e) {
    setStatus(offlineStatus, { tone: "bad", message: String(e) });
  }
  renderOffline(await invoke<OfflineStatus>("speech_offline_status"));
});

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

  const speechModels = await invoke<string[]>("list_speech_models");
  speechModel.replaceChildren(
    ...speechModels.map((name) => {
      const o = document.createElement("option");
      o.value = name;
      o.textContent = name;
      return o;
    }),
  );

  const regions = await invoke<{ id: string; label: string }[]>("list_azure_regions");
  azureRegion.replaceChildren(
    option("", "Choose a region…"),
    ...regions.map((r) => option(r.id, r.label)),
  );

  const languages = await invoke<string[]>("list_languages");
  voiceLang.replaceChildren(...languages.map((name) => option(name, name)));
  langList.replaceChildren(
    ...languages.map((name) => {
      const o = document.createElement("option");
      o.value = name;
      return o;
    }),
  );

  renderForm();
  voiceLang.value = languages.includes(settings.targetLang) ? settings.targetLang : "English";
  void loadVoices();
  wireHotkey(hkText, el<HTMLElement>("hkTextStatus"), "text");
  wireHotkey(hkReplace, el<HTMLElement>("hkReplaceStatus"), "replace");
  wireHotkey(hkShot, el<HTMLElement>("hkShotStatus"), "screenshot");

  const report = await invoke<PlatformReport>("platform_report");
  platform.textContent = `${report.backend}
selection ${report.supportsTextSelection ? "supported" : "unavailable"}  ·  capture ${
    report.supportsScreenCapture ? "supported" : "unavailable"
  }  ·  rewrite in place ${report.supportsReplace ? "supported" : "unavailable"}`;
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
  el<HTMLElement>("keyNote").textContent =
    `Kept in ${report.keyStorage}, not in the settings file. Never logged, and only ever ` +
    "sent to the Base URL above.";
  if (!report.supportsSelectionBubble) {
    selectionBubble.disabled = true;
    el<HTMLElement>("selectionBubbleNote").textContent =
      "Not available on this system yet — it needs a global mouse hook, which only the Windows build has.";
  }

  await listen<OfflineStatus>("speech:offline", (e) => renderOffline(e.payload));
  renderOffline(await invoke<OfflineStatus>("speech_offline_status"));

  await loadLog();
  saveState.textContent = "UP TO DATE";
}

void boot().catch((e) => {
  saveState.textContent = `FAILED TO LOAD — ${String(e)}`;
  saveState.style.color = "var(--danger)";
});

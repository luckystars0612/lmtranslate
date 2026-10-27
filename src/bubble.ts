import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

// This window never takes focus — the click is followed by a synthetic Ctrl+C that has
// to reach the application holding the selection. So buttons act on pointerdown with
// the default prevented (no focus moves inside the page either), and nothing here
// depends on keyboard input.

const pill = document.getElementById("pill") as HTMLElement;

function wire(id: string, action: "translate" | "rewrite") {
  const button = document.getElementById(id) as HTMLButtonElement;
  button.addEventListener("pointerdown", (e) => {
    if (e.button !== 0) return;
    e.preventDefault();
    void invoke("bubble_action", { action });
  });
}

wire("translate", "translate");
wire("rewrite", "rewrite");

// Hovering keeps the bubble; leaving starts a short countdown in Rust.
document.documentElement.addEventListener("pointerenter", () => {
  void invoke("bubble_hover", { inside: true });
});
document.documentElement.addEventListener("pointerleave", () => {
  void invoke("bubble_hover", { inside: false });
});

window.addEventListener("contextmenu", (e) => e.preventDefault());

async function applyTheme() {
  try {
    const theme = await invoke<string>("app_theme");
    if (theme === "light" || theme === "dark") document.documentElement.dataset.theme = theme;
    else delete document.documentElement.dataset.theme;
  } catch {
    // Keep whatever theme is already applied.
  }
}

// The window is reused, so replay the entrance and pick up a theme change each time.
void listen("bubble:show", () => {
  pill.classList.remove("in");
  void pill.offsetWidth;
  pill.classList.add("in");
  void applyTheme();
});

void applyTheme();

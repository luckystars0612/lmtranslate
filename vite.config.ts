import { defineConfig } from "vite";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = fileURLToPath(new URL(".", import.meta.url));

// Multi-page build: four independent, tiny documents (popup, crop overlay, selection
// bubble, settings).
// There is no "main" window in this app, so there is no index.html.
export default defineConfig({
  root: "src",
  base: "./",
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  build: {
    outDir: "../dist",
    emptyOutDir: true,
    target: "chrome105",
    rollupOptions: {
      input: {
        popup: resolve(here, "src/popup.html"),
        settings: resolve(here, "src/settings.html"),
        "crop-overlay": resolve(here, "src/crop-overlay.html"),
        bubble: resolve(here, "src/bubble.html"),
      },
    },
  },
});

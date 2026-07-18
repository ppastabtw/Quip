import { defineConfig } from "vite";
import { resolve } from "node:path";

// The Tauri webviews load three separate pages out of src/ui.
export default defineConfig({
  root: "src/ui",
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
  },
  build: {
    outDir: resolve(import.meta.dirname, "dist"),
    emptyOutDir: true,
    rollupOptions: {
      input: {
        suggestions: resolve(import.meta.dirname, "src/ui/suggestions.html"),
        settings: resolve(import.meta.dirname, "src/ui/settings.html"),
        demo: resolve(import.meta.dirname, "src/ui/demo.html"),
      },
    },
  },
});

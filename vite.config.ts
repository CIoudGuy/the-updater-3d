import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

export default defineConfig({
  plugins: [svelte()],
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  build: {
    target: "chrome120",
    outDir: "dist",
    emptyOutDir: true,
    rollupOptions: { input: { main: "index.html", tray: "tray.html" } },
  },
});

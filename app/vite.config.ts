import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// Porta fixa: o Tauri procura a interface em devUrl.
export default defineConfig({
  plugins: [svelte()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    // Os tokens de design ficam em ../design e são a única fonte das cores.
    fs: { allow: [".."] },
  },
  // O pacote vem do disco junto com o app, sem rede: um arquivo só de ~550 kB não atrasa a abertura.
  build: { target: "es2022", chunkSizeWarningLimit: 800 },
});

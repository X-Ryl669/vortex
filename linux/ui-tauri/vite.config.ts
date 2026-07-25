import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";
import path from "path";

export default defineConfig(async () => ({
  plugins: [
    vue({
      template: {
        compilerOptions: {
          // <emoji-picker> is a web component (emoji-picker-element), not a
          // Vue component — stop Vue from trying to resolve it.
          isCustomElement: (tag) => tag === "emoji-picker",
        },
      },
    }),
  ],
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },
  clearScreen: false,
  server: {
    // 5172, NOT Vite's default 5173: the dev machine often has another
    // Vite project on 5173, and Tauri dev would render THAT app instead
    // of Vortex. strictPort keeps the failure loud if 5172 is taken too.
    port: 5172,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/**"] },
  },
}));

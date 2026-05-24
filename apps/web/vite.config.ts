import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import path from "node:path";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: { "@": path.resolve(__dirname, "src") },
  },
  server: {
    proxy: {
      "/health": "http://127.0.0.1:17777",
      "/sessions": "http://127.0.0.1:17777",
      "/runs": "http://127.0.0.1:17777",
    },
  },
  test: {
    environment: "jsdom",
    globals: true,
  },
});

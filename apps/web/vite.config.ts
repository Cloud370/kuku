import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import path from "node:path";

const server = process.env.KUKU_WEB_DEV_SERVER ?? "http://127.0.0.1:17777";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: { "@": path.resolve(import.meta.dirname, "src") },
  },
  server: {
    proxy: Object.fromEntries(
      ["/health", "/service", "/sessions", "/workspaces", "/models"].map((prefix) => [
        prefix,
        server,
      ]),
    ),
  },
  test: {
    environment: "jsdom",
    globals: true,
  },
});

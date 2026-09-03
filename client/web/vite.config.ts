import { defineConfig } from "vite";

// `host: true` exposes the dev server on the LAN so the Android device (phase
// 2) and other machines can load it too.
export default defineConfig({
  server: { host: true, port: 5173 },
  preview: { host: true, port: 4173 },
});

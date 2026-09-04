import { defineConfig } from "vite";
import { viteSingleFile } from "vite-plugin-singlefile";

// Build to a single self-contained index.html (all JS/CSS inlined). This lets
// the Android wrapper load it straight from `file://` — no ES-module CORS
// failure, and `file://` -> `http://<lan>` API calls are not "mixed content".
export default defineConfig({
  base: "./",
  plugins: [viteSingleFile()],
  server: { host: true, port: 5173 },
  preview: { host: true, port: 4173 },
});

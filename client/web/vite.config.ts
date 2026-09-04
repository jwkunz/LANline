import { defineConfig, type Plugin } from "vite";

// `base: "./"` so the built asset paths are relative — the same bundle works
// served from a web root, from a sub-path, and from `file://` inside the
// Android WebView. Stripping `crossorigin` avoids a CORS check that some
// Android WebView versions fail on `file://` module scripts.
function fileProtocolFriendly(): Plugin {
  return {
    name: "file-protocol-friendly",
    transformIndexHtml(html) {
      return html.replace(/\s+crossorigin(=("|')[^"']*\2)?/g, "");
    },
  };
}

export default defineConfig({
  base: "./",
  plugins: [fileProtocolFriendly()],
  server: { host: true, port: 5173 },
  preview: { host: true, port: 4173 },
});

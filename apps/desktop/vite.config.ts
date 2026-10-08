import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

const viteCacheDir = "node_modules/.vite-tendi-dev";

export default defineConfig({
  optimizeDeps: {
    include: ["react", "react-dom/client"],
  },
  cacheDir: viteCacheDir,
  server: {
    // WKWebView persists optimized modules across dev runs; always fetch the current graph.
    headers: { "Cache-Control": "no-store" },
    proxy: {
      "/__tendi": "http://127.0.0.1:5188",
    },
    warmup: {
      clientFiles: ["./src/main.tsx"],
    },
  },
  plugins: [react()],
});

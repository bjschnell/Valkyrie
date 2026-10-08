import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// `npm run dev` proxies the WebSocket to a running `valk web` (default port).
export default defineConfig({
  plugins: [react()],
  build: { outDir: "dist", emptyOutDir: true, sourcemap: false },
  server: {
    proxy: { "/ws": { target: "ws://127.0.0.1:8790", ws: true }, "/api": "http://127.0.0.1:8790" },
  },
});

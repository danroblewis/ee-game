import { defineConfig } from 'vite';

export default defineConfig({
  build: {
    // Top-level await (WASM init) needs a modern baseline.
    target: 'es2022',
  },
  server: {
    // Allow access through a quick cloudflared tunnel (dev only).
    allowedHosts: ['.trycloudflare.com'],
    proxy: {
      // Dev-mode: the game server owns /ws (run `cargo run -p server`).
      // EE_SERVER_PORT points a dev client at a server other than the default
      // one, so isolated rooms can run side by side without disturbing it.
      // It has to cover BOTH proxies or the lobby talks to a different server
      // than the simulation does.
      '/ws': { target: `ws://localhost:${process.env.EE_SERVER_PORT ?? 8080}`, ws: true },
      // ...and /api, the room lobby (create / choose / delete rooms and
      // templates). Plain HTTP, so no `ws: true` here.
      '/api': { target: `http://localhost:${process.env.EE_SERVER_PORT ?? 8080}` },
    },
  },
});

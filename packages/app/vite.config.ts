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
      // Override the server port with EE_SERVER_PORT to run parallel instances.
      '/ws': { target: `ws://localhost:${process.env.EE_SERVER_PORT ?? 8080}`, ws: true },
    },
  },
});

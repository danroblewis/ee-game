import { defineConfig } from 'vite';

export default defineConfig({
  build: {
    // Top-level await (WASM init) needs a modern baseline.
    target: 'es2022',
  },
  server: {
    proxy: {
      // Dev-mode: the game server owns /ws (run `cargo run -p server`).
      // EE_SERVER_PORT points a dev client at a server other than the
      // default one, so two people can run isolated rooms side by side.
      '/ws': { target: `ws://localhost:${process.env.EE_SERVER_PORT ?? 8080}`, ws: true },
    },
  },
});

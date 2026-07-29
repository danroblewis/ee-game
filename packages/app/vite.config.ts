import { defineConfig } from 'vite';

export default defineConfig({
  build: {
    // Top-level await (WASM init) needs a modern baseline.
    target: 'es2022',
  },
  server: {
    proxy: {
      // Dev-mode: the game server owns /ws (run `cargo run -p server`).
      '/ws': { target: 'ws://localhost:8080', ws: true },
    },
  },
});

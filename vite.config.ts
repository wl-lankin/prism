import { defineConfig } from 'vitest/config';
import { svelte } from '@sveltejs/vite-plugin-svelte';

export default defineConfig({
  plugins: [svelte()],
  clearScreen: false,
  server: { port: 1420, strictPort: true, watch: { ignored: ['**/src-tauri/**'] } },
  envPrefix: ['VITE_', 'TAURI_ENV_'],
  build: { outDir: 'dist', emptyOutDir: true, target: 'chrome120', modulePreload: { polyfill: false } },
  // Under vitest, resolve Svelte's browser build: the server build turns $effect, flushSync and untrack into no-ops.
  resolve: process.env.VITEST ? { conditions: ['browser'] } : undefined,
  test: {
    environment: 'jsdom',
    include: ['src/**/*.spec.ts'],
  },
});

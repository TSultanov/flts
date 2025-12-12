import { defineConfig } from 'vite'
import { svelte } from '@sveltejs/vite-plugin-svelte'
import { resolve } from 'path'

// https://vite.dev/config/
export default defineConfig({
  plugins: [
    svelte(),
  ],
  resolve: {
    alias: {
      // Mock Tauri APIs when running Playwright tests
      ...(process.env.PLAYWRIGHT && {
        '@tauri-apps/api/core': resolve(__dirname, 'tests/mocks/tauri-api.ts'),
        '@tauri-apps/api/event': resolve(__dirname, 'tests/mocks/tauri-event.ts'),
        '@tauri-apps/plugin-dialog': resolve(__dirname, 'tests/mocks/tauri-dialog.ts'),
      }),
    },
  },
  server: {
    headers: {
      'Cross-Origin-Opener-Policy': 'same-origin',
      'Cross-Origin-Embedder-Policy': 'require-corp',
    },
    host: process.env.TAURI_DEV_HOST || 'localhost',
  },
  optimizeDeps: {
    exclude: ['@sqlite.org/sqlite-wasm'],
  },
})

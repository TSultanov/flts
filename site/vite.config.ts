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
        '@tauri-apps/plugin-os': resolve(__dirname, 'tests/mocks/tauri-os.ts'),
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
    exclude: [
      '@sqlite.org/sqlite-wasm',
      // When running under Playwright, exclude every Tauri plugin from
      // Vite's pre-bundling so that nested `import '@tauri-apps/api/...'`
      // calls inside the plugins resolve through our alias instead of
      // embedding a snapshot of the real (or stale-mock) module. Without
      // this, listen/emit and mock state Maps end up split across two
      // module instances and events get lost.
      ...(process.env.PLAYWRIGHT
        ? [
            '@tauri-apps/api',
            '@tauri-apps/api/core',
            '@tauri-apps/api/event',
            '@tauri-apps/plugin-dialog',
            '@tauri-apps/plugin-log',
            '@tauri-apps/plugin-os',
            '@tauri-apps/plugin-window-state',
          ]
        : []),
    ],
  },
})

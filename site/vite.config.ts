import { defineConfig } from 'vite'
import { svelte } from '@sveltejs/vite-plugin-svelte'
import { resolve } from 'path'

export default defineConfig({
  plugins: [
    svelte(),
  ],
  resolve: {
    alias: {
      ...(process.env.PLAYWRIGHT && {
        '@tauri-apps/api/core': resolve(__dirname, 'tests/mocks/tauri-api.ts'),
        '@tauri-apps/api/event': resolve(__dirname, 'tests/mocks/tauri-event.ts'),
        '@tauri-apps/plugin-dialog': resolve(__dirname, 'tests/mocks/tauri-dialog.ts'),
        '@tauri-apps/plugin-os': resolve(__dirname, 'tests/mocks/tauri-os.ts'),
      }),
      // Real tier: core/event go over the WS bridge; dialog/os stay mocked —
      // native pickers have no headless equivalent.
      ...(process.env.PLAYWRIGHT_REAL && {
        '@tauri-apps/api/core': resolve(__dirname, 'tests/real/tauri-shim-core.ts'),
        '@tauri-apps/api/event': resolve(__dirname, 'tests/real/tauri-shim-event.ts'),
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
      // Pre-bundling would snapshot the plugins' nested `@tauri-apps/api/*`
      // imports past the aliases above, splitting mock state / the bridge
      // socket across two module instances.
      ...(process.env.PLAYWRIGHT || process.env.PLAYWRIGHT_REAL
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

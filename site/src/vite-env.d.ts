/// <reference types="svelte" />
/// <reference types="vite/client" />

import type { debug } from './lib/debug';

declare global {
  interface Window {
    debug: typeof debug;
  }
}

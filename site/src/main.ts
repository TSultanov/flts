import { mount } from 'svelte'
import './app.css'
import App from './App.svelte'
import { debug } from './lib/debug'

const app = mount(App, {
  target: document.getElementById('app')!,
})

// Register debug instance globally for dev console access
if (typeof window !== 'undefined') {
  (window as any).debug = debug;
}

export default app

import { mount } from 'svelte'
import './app.css'
import App from './App.svelte'
import { attachConsole } from '@tauri-apps/plugin-log';

await attachConsole();

const app = mount(App, {
  target: document.getElementById('app')!,
})

export default app

import { mount } from 'svelte'
import './app.css'
import App from './App.svelte'
import { apply } from './lib/settings.svelte'

apply()

const app = mount(App, {
  target: document.getElementById('app')!,
})

export default app

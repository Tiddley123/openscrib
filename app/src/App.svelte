<script lang="ts">
  import { listen } from '@tauri-apps/api/event'
  import { getCurrentWindow } from '@tauri-apps/api/window'
  import { onMount } from 'svelte'
  import { api, type ProjectView } from './lib/api'
  import Editor from './lib/Editor.svelte'
  import Home from './lib/Home.svelte'
  import SettingsDialog from './lib/SettingsDialog.svelte'

  let project = $state<ProjectView | null>(null)
  let recording = $state(false)
  let error = $state<string | null>(null)
  let busy = $state<string | null>(null)
  let settingsOpen = $state(false)

  async function open(path: string) {
    error = null
    busy = 'Opening project…'
    try {
      project = await api.openProject(path)
    } catch (e) {
      error = String(e)
    } finally {
      busy = null
    }
  }

  async function startRecording(req: Parameters<typeof api.startRecording>[0]) {
    error = null
    try {
      await api.startRecording(req)
      recording = true
      // Get out of the way of whatever is being recorded.
      await getCurrentWindow().minimize()
    } catch (e) {
      error = String(e)
    }
  }

  onMount(() => {
    const un = [
      listen<string>('recording-finished', async (e) => {
        recording = false
        const w = getCurrentWindow()
        await w.unminimize()
        await w.setFocus()
        await open(e.payload)
      }),
      listen<string>('recording-failed', async (e) => {
        recording = false
        await getCurrentWindow().unminimize()
        error = e.payload
      }),
    ]
    return () => un.forEach((p) => p.then((f) => f()))
  })
</script>

{#if recording}
  <main class="recording">
    <div class="pulse"></div>
    <h1>Recording</h1>
    <p>Click and type through the task. Press <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>F12</kbd> or Stop when you're done.</p>
    <button class="primary" onclick={() => api.stopRecording()}>Stop recording</button>
  </main>
{:else if project}
  <Editor bind:project onclose={() => (project = null)} onsettings={() => (settingsOpen = true)} />
{:else}
  <Home onopen={open} onrecord={startRecording} onsettings={() => (settingsOpen = true)} />
{/if}

{#if settingsOpen}
  <SettingsDialog onclose={() => (settingsOpen = false)} />
{/if}

{#if busy}
  <div class="toast">{busy}</div>
{/if}
{#if error}
  <div class="toast error" role="alert">
    {error}
    <button class="ghost" onclick={() => (error = null)}>Dismiss</button>
  </div>
{/if}

<style>
  .recording {
    height: 100%;
    display: grid;
    place-content: center;
    justify-items: center;
    gap: 12px;
    text-align: center;
    padding: 24px;
  }
  .recording h1 {
    margin: 8px 0 0;
    font-size: 28px;
    letter-spacing: -0.02em;
  }
  .recording p {
    color: var(--ink-2);
    max-width: 420px;
    margin: 0 0 12px;
  }
  .pulse {
    width: 18px;
    height: 18px;
    border-radius: 50%;
    background: var(--accent);
    box-shadow: 0 0 0 0 color-mix(in srgb, var(--accent) 50%, transparent);
    animation: pulse 1.6s infinite;
  }
  @keyframes pulse {
    to {
      box-shadow: 0 0 0 18px transparent;
    }
  }
  .toast {
    position: fixed;
    left: 50%;
    bottom: 20px;
    transform: translateX(-50%);
    background: var(--ink);
    color: var(--bg);
    padding: 10px 14px;
    border-radius: var(--radius-sm);
    box-shadow: var(--shadow);
    display: flex;
    align-items: center;
    gap: 12px;
    max-width: min(720px, calc(100% - 32px));
    z-index: 50;
  }
  .toast.error {
    background: var(--danger);
    color: #fff;
  }
  .toast button {
    color: inherit;
  }
</style>

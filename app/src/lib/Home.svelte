<script lang="ts">
  import { open as openDialog } from '@tauri-apps/plugin-dialog'
  import { onMount } from 'svelte'
  import { api, formatTime, type Devices, type RecentItem, type RecordRequest } from './api'
  import { settings } from './settings.svelte'

  let { onopen, onrecord, onsettings }: {
    onopen: (path: string) => void
    onrecord: (req: RecordRequest) => void
    onsettings: () => void
  } = $props()

  let recents = $state<RecentItem[]>([])
  let devices = $state<Devices | null>(null)
  let video = $state(true)
  let audio = $state(true)
  let monitor = $state<number | null>(null)
  let mic = $state<string | null>(null)

  onMount(async () => {
    recents = await api.listRecordings().catch(() => [])
    devices = await api.listDevices().catch(() => null)
    if (devices) {
      monitor = devices.monitors.find((m) => m.primary)?.id ?? devices.monitors[0]?.id ?? null
      video = devices.video_available
    }
  })

  async function browse() {
    const path = await openDialog({ multiple: false, filters: [{ name: 'OpenSCRIB project', extensions: ['scrib'] }] })
    if (typeof path === 'string') onopen(path)
  }

  function ago(secs: number): string {
    const d = Date.now() / 1000 - secs
    if (d < 60) return 'just now'
    if (d < 3600) return `${Math.floor(d / 60)} min ago`
    if (d < 86400) return `${Math.floor(d / 3600)} h ago`
    return new Date(secs * 1000).toLocaleDateString()
  }
</script>

<main>
  <header>
    <div class="brand"><span class="dot"></span>OpenSCRIB</div>
    <div class="head-actions">
      <button class="ghost" onclick={onsettings}>Settings</button>
      <button onclick={browse}>Open project…</button>
    </div>
  </header>

  <section class="hero">
    <div>
      <h1>Record once.<br />Export a guide and a video.</h1>
      <p>Clicks, keystrokes, screen and narration are captured locally. Edit the steps, then export Markdown, HTML, PDF, MKV or MP4.</p>
    </div>

    <div class="card record">
      <div class="label">New recording</div>
      <div class="opts">
        <label class="toggle">
          <input type="checkbox" bind:checked={video} disabled={!devices?.video_available} />
          Screen video
        </label>
        {#if devices && video && devices.monitors.length > 1}
          <select bind:value={monitor}>
            {#each devices.monitors as m (m.id)}
              <option value={m.id}>{m.name} · {m.rect.w}×{m.rect.h}{m.primary ? ' · primary' : ''}</option>
            {/each}
          </select>
        {/if}
        <label class="toggle">
          <input type="checkbox" bind:checked={audio} />
          Narration
        </label>
        {#if devices && audio && devices.mics.length > 0}
          <select bind:value={mic}>
            <option value={null}>Default microphone</option>
            {#each devices.mics as [name] (name)}
              <option value={name}>{name}</option>
            {/each}
          </select>
        {/if}
        {#if devices && !devices.video_available}
          <p class="hint">Screen video needs <code>openh264-2.6.0-win64.dll</code> beside the app or in <code>runtime/</code>.</p>
        {/if}
      </div>
      <button class="primary big" onclick={() => onrecord({ video, audio, monitor, mic, quality: settings.quality, stop_key: settings.stopKey })}>
        <span class="rec"></span> Start recording
      </button>
      <p class="hint">The window minimises while you record. Stop with {#each settings.stopKey.split("+") as k, i}{#if i}+{/if}<kbd>{k}</kbd>{/each}.</p>
    </div>
  </section>

  <section>
    <div class="label">Recent recordings</div>
    {#if recents.length === 0}
      <p class="empty">Nothing yet. Your recordings will show up here.</p>
    {:else}
      <ul class="recents">
        {#each recents as r (r.path)}
          <li>
            <button class="item" onclick={() => onopen(r.path)}>
              <span class="name">{r.title ?? r.name}</span>
              <span class="meta">{r.steps} steps · {formatTime(r.duration_ms)} · {ago(r.modified)}</span>
            </button>
          </li>
        {/each}
      </ul>
    {/if}
  </section>
</main>

<style>
  main {
    height: 100%;
    overflow: auto;
    padding: 0 48px 48px;
    max-width: 1180px;
    margin: 0 auto;
  }
  header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 20px 0;
  }
  .head-actions {
    display: flex;
    gap: 8px;
  }
  .brand {
    display: flex;
    align-items: center;
    gap: 10px;
    font-weight: 700;
    letter-spacing: -0.01em;
    font-size: 16px;
  }
  .dot {
    width: 14px;
    height: 14px;
    border-radius: 50%;
    border: 3px solid var(--accent);
  }
  .hero {
    display: grid;
    grid-template-columns: 1.1fr 1fr;
    gap: 48px;
    align-items: center;
    padding: 56px 0 64px;
  }
  h1 {
    font-size: 44px;
    line-height: 1.05;
    letter-spacing: -0.035em;
    margin: 0 0 16px;
  }
  .hero p {
    color: var(--ink-2);
    font-size: 16px;
    max-width: 460px;
    margin: 0;
  }
  .card {
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: 16px;
    padding: 24px;
    box-shadow: var(--shadow);
  }
  .record {
    display: grid;
    gap: 16px;
  }
  .opts {
    display: grid;
    gap: 10px;
  }
  .toggle {
    display: flex;
    align-items: center;
    gap: 10px;
    font-weight: 500;
  }
  .toggle input {
    accent-color: var(--accent);
    width: 16px;
    height: 16px;
  }
  .big {
    justify-content: center;
    padding: 12px 16px;
    font-size: 15px;
  }
  .rec {
    width: 10px;
    height: 10px;
    border-radius: 50%;
    background: currentColor;
  }
  .hint {
    color: var(--muted);
    font-size: 12.5px;
    margin: 0;
  }
  .empty {
    color: var(--muted);
  }
  .recents {
    list-style: none;
    margin: 12px 0 0;
    padding: 0;
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(280px, 1fr));
    gap: 12px;
  }
  .item {
    width: 100%;
    display: grid;
    gap: 4px;
    text-align: left;
    padding: 14px 16px;
    border-radius: var(--radius);
    border-color: var(--line);
  }
  .name {
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .meta {
    color: var(--muted);
    font-size: 12.5px;
  }
  @media (max-width: 900px) {
    .hero {
      grid-template-columns: 1fr;
    }
  }
</style>

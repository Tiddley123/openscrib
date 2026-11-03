<script lang="ts">
  import { listen } from '@tauri-apps/api/event'
  import { onMount } from 'svelte'
  import { api, type ModelStatus } from './api'
  import { settings } from './settings.svelte'

  let models = $state<ModelStatus[]>([])
  let progress = $state<Record<string, number>>({})
  let errors = $state<Record<string, string>>({})

  let whisper = $derived(models.filter((m) => m.kind === 'whisper'))
  let ocr = $derived(models.filter((m) => m.kind === 'ocr_detection' || m.kind === 'ocr_recognition'))
  let vision = $derived(models.filter((m) => m.kind === 'vision_model' || m.kind === 'vision_projector'))
  let engine = $derived(models.find((m) => m.kind === 'tts_engine'))
  let voices = $derived(models.filter((m) => m.kind === 'tts_voice'))
  let visionReady = $derived(vision.length > 0 && vision.every((m) => m.installed))
  let visionSize = $derived(vision.reduce((n, m) => n + m.size_mb, 0))
  let ocrReady = $derived(ocr.length > 0 && ocr.every((m) => m.installed))
  let ocrSize = $derived(ocr.reduce((n, m) => n + m.size_mb, 0))

  async function refresh() {
    models = await api.listModels().catch(() => [])
  }

  onMount(() => {
    refresh()
    const un = [
      listen<{ id: string; done: number; total: number | null }>('model-progress', (e) => {
        const { id, done, total } = e.payload
        progress[id] = total ? done / total : 0
      }),
      listen<string>('model-done', async (e) => {
        delete progress[e.payload]
        await refresh()
        // Choosing the first Whisper model you download saves a step.
        const m = models.find((x) => x.id === e.payload)
        if (m?.kind === 'whisper' && !models.some((x) => x.kind === 'whisper' && x.installed && x.id === settings.whisperModel)) {
          settings.whisperModel = m.id
        }
      }),
      listen<{ id: string; error: string }>('model-failed', (e) => {
        delete progress[e.payload.id]
        errors[e.payload.id] = e.payload.error
      }),
    ]
    return () => un.forEach((p) => p.then((f) => f()))
  })

  async function download(ids: string[]) {
    for (const id of ids) {
      delete errors[id]
      progress[id] = 0
      await api.downloadModel(id).catch((e) => {
        delete progress[id]
        errors[id] = String(e)
      })
    }
  }

  async function remove(id: string) {
    await api.removeModel(id).catch((e) => (errors[id] = String(e)))
    await refresh()
  }

  function size(mb: number) {
    return mb >= 1000 ? `${(mb / 1000).toFixed(1)} GB` : `${mb} MB`
  }
</script>

<section>
  <div class="label">Narration (Whisper)</div>
  <p class="hint">Turns what you say while recording into text for each step. Runs on this PC; pick one to use and download it once.</p>
  <ul>
    {#each whisper as m (m.id)}
      <li class:active={settings.whisperModel === m.id && m.installed}>
        <label>
          <input
            type="radio"
            name="whisper"
            value={m.id}
            bind:group={settings.whisperModel}
            disabled={!m.installed}
          />
          <span class="name">{m.label}</span>
          <span class="note">{m.note} · {size(m.size_mb)}</span>
        </label>
        {#if progress[m.id] !== undefined}
          <span class="pct">{Math.round(progress[m.id] * 100)}%</span>
        {:else if m.installed}
          <button class="ghost small" onclick={() => remove(m.id)}>Remove</button>
        {:else}
          <button class="small" onclick={() => download([m.id])}>Download</button>
        {/if}
        {#if progress[m.id] !== undefined}
          <span class="bar"><span style:width="{progress[m.id] * 100}%"></span></span>
        {/if}
        {#if errors[m.id]}<span class="error">{errors[m.id]}</span>{/if}
      </li>
    {/each}
  </ul>
</section>

<section>
  <div class="label">On-screen text (OCR)</div>
  <p class="hint">Reads the label under each click and any tooltip, so steps say "Click <b>This PC</b>" instead of naming the window.</p>
  <ul>
    <li class:active={ocrReady}>
      <span class="name">{ocrReady ? 'Installed' : 'Not installed'}</span>
      <span class="note">Two small models · {size(ocrSize)}</span>
      {#if ocr.some((m) => progress[m.id] !== undefined)}
        <span class="pct">Downloading…</span>
      {:else if ocrReady}
        <button class="ghost small" onclick={() => ocr.forEach((m) => remove(m.id))}>Remove</button>
      {:else}
        <button class="small" onclick={() => download(ocr.filter((m) => !m.installed).map((m) => m.id))}>Download</button>
      {/if}
      {#each ocr as m}
        {#if errors[m.id]}<span class="error">{errors[m.id]}</span>{/if}
      {/each}
    </li>
  </ul>
</section>

<section>
  <div class="label">Vision &amp; writing (Qwen2.5-VL 3B)</div>
  <p class="hint">Names icons with no text (like the Start button), picks the right label when OCR reads the wrong line, works your narration into the step text, and titles the guide. Runs on this PC, about 20–40 s per recording on CPU.</p>
  <ul>
    <li class:active={visionReady}>
      <span class="name">{visionReady ? 'Installed' : 'Not installed'}</span>
      <span class="note">Model and image encoder · {size(visionSize)}</span>
      {#if vision.some((m) => progress[m.id] !== undefined)}
        {@const p = vision.map((m) => progress[m.id] ?? (m.installed ? 1 : 0))}
        <span class="pct">{Math.round(((p[0] + p[1]) / 2) * 100)}%</span>
      {:else if visionReady}
        <button class="ghost small" onclick={() => vision.forEach((m) => remove(m.id))}>Remove</button>
      {:else}
        <button class="small" onclick={() => download(vision.filter((m) => !m.installed).map((m) => m.id))}>Download</button>
      {/if}
      {#each vision as m}
        {#if errors[m.id]}<span class="error">{errors[m.id]}</span>{/if}
      {/each}
    </li>
  </ul>
</section>

<section>
  <div class="label">Voiceover (Piper)</div>
  <p class="hint">Reads each step's text aloud in exported videos, instead of your recorded narration. Needs the engine and one voice.</p>
  <ul>
    {#if engine}
      <li class:active={engine.installed}>
        <span class="name">{engine.label}</span>
        <span class="note">{engine.note} · {size(engine.size_mb)}</span>
        {#if progress[engine.id] !== undefined}
          <span class="pct">{Math.round(progress[engine.id] * 100)}%</span>
        {:else if engine.installed}
          <button class="ghost small" onclick={() => remove(engine!.id)}>Remove</button>
        {:else}
          <button class="small" onclick={() => download([engine!.id])}>Download</button>
        {/if}
        {#if errors[engine.id]}<span class="error">{errors[engine.id]}</span>{/if}
      </li>
    {/if}
    {#each voices as m (m.id)}
      <li class:active={settings.voice === m.id && m.installed}>
        <label>
          <input type="radio" name="voice" value={m.id} bind:group={settings.voice} disabled={!m.installed} />
          <span class="name">{m.label}</span>
          <span class="note">{m.note} · {size(m.size_mb)}</span>
        </label>
        {#if progress[m.id] !== undefined}
          <span class="pct">{Math.round(progress[m.id] * 100)}%</span>
        {:else if m.installed}
          <button class="ghost small" onclick={() => remove(m.id)}>Remove</button>
        {:else}
          <button class="small" onclick={() => download([m.id])}>Download</button>
        {/if}
        {#if errors[m.id]}<span class="error">{errors[m.id]}</span>{/if}
      </li>
    {/each}
  </ul>
</section>

<style>
  section {
    display: grid;
    gap: 8px;
  }
  .hint {
    margin: 0;
    font-size: 12.5px;
    color: var(--muted);
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 4px;
  }
  li {
    position: relative;
    display: grid;
    grid-template-columns: 1fr auto;
    align-items: center;
    gap: 2px 10px;
    padding: 8px 10px;
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
    overflow: hidden;
  }
  li.active {
    border-color: color-mix(in srgb, var(--accent) 45%, transparent);
    background: var(--accent-soft);
  }
  label {
    display: grid;
    grid-template-columns: 18px 1fr;
    column-gap: 8px;
    align-items: center;
  }
  label input {
    accent-color: var(--accent);
    grid-row: span 2;
  }
  .name {
    font-weight: 600;
    font-size: 13px;
  }
  .note {
    color: var(--muted);
    font-size: 12px;
  }
  li > .name,
  li > .note {
    grid-column: 1;
  }
  li > button,
  li > .pct {
    grid-column: 2;
    grid-row: 1 / span 2;
  }
  .small {
    padding: 4px 10px;
    font-size: 12.5px;
  }
  .pct {
    font-size: 12.5px;
    font-variant-numeric: tabular-nums;
    color: var(--ink-2);
  }
  .bar {
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
    height: 3px;
    background: var(--line);
  }
  .bar span {
    display: block;
    height: 100%;
    background: var(--accent);
    transition: width 0.3s;
  }
  .error {
    grid-column: 1 / -1;
    color: var(--danger);
    font-size: 12px;
  }
</style>

<script lang="ts">
  import { listen } from '@tauri-apps/api/event'
  import { open as openDialog } from '@tauri-apps/plugin-dialog'
  import { openPath, revealItemInDir } from '@tauri-apps/plugin-opener'
  import { onMount } from 'svelte'
  import { api, type ExportFormat, type Manifest } from './api'
  import { qualityLabel, settings } from './settings.svelte'

  let { manifest, hasVideo, onclose, onsaved }: {
    manifest: Manifest
    hasVideo: boolean
    onclose: () => void
    onsaved: (json: string) => void
  } = $props()

  const formats: { id: ExportFormat; label: string; note: string; video?: boolean }[] = [
    { id: 'pdf', label: 'PDF', note: 'Print-ready guide' },
    { id: 'html', label: 'HTML', note: 'One self-contained page' },
    { id: 'md', label: 'Markdown', note: 'guide.md + images folder' },
    { id: 'mp4', label: 'MP4', note: 'Plays everywhere, incl. QuickTime and PowerPoint', video: true },
    { id: 'mkv', label: 'MKV', note: 'Open format, same video', video: true },
  ]

  // svelte-ignore state_referenced_locally
  let chosen = $state<Record<ExportFormat, boolean>>({ pdf: true, html: true, md: false, mp4: hasVideo, mkv: false })
  let fullFrame = $state(false)
  let voiceover = $state(false)
  let voiceReady = $state(false)
  let outDir = $state('')
  let running = $state(false)
  let progress = $state<Record<string, number>>({})
  let done = $state<{ out_dir: string; files: string[] } | null>(null)
  let error = $state<string | null>(null)

  let selected = $derived(formats.filter((f) => chosen[f.id] && (!f.video || hasVideo)).map((f) => f.id))
  let clipCount = $derived(manifest.steps.filter((s) => s.clip).length)

  onMount(() => {
    // Voiceover needs either a voice built into the system or Piper and one
    // of its voices downloaded.
    Promise.all([api.listModels(), api.listVoices()])
      .then(([ms, vs]) => {
        voiceReady =
          vs.length > 0 ||
          (ms.some((m) => m.kind === 'tts_engine' && m.installed) && ms.some((m) => m.kind === 'tts_voice' && m.installed))
      })
      .catch(() => {})
    api.defaultExportDir(manifest.title ?? null).then((d) => (outDir = d))
    const un = [
      listen<{ format: string; progress: number }>('export-progress', (e) => {
        progress[e.payload.format] = e.payload.progress
      }),
      listen<{ out_dir: string; files: string[] }>('export-done', (e) => {
        running = false
        done = e.payload
      }),
      listen<string>('export-failed', (e) => {
        running = false
        error = e.payload
      }),
    ]
    return () => un.forEach((p) => p.then((f) => f()))
  })

  async function browse() {
    const d = await openDialog({ directory: true, defaultPath: outDir || undefined })
    if (typeof d === 'string') outDir = d
  }

  async function run() {
    error = null
    done = null
    progress = {}
    running = true
    try {
      const snapshot = $state.snapshot(manifest)
      await api.exportProject({
        manifest: snapshot,
        formats: selected,
        out_dir: outDir,
        full_frame: fullFrame,
        quality: settings.quality,
        max_width: settings.exportWidth,
        voiceover: voiceover && voiceReady ? settings.voice : null,
      })
      // Exporting saves the project first.
      onsaved(JSON.stringify(snapshot))
    } catch (e) {
      running = false
      error = String(e)
    }
  }
</script>

<div
  class="scrim"
  role="presentation"
  onclick={(e) => {
    if (e.target === e.currentTarget && !running) onclose()
  }}
>
  <div class="dialog" role="dialog" aria-modal="true" aria-labelledby="export-title">
    <h2 id="export-title">Export</h2>

    <div class="formats">
      {#each formats as f (f.id)}
        {@const disabled = f.video && !hasVideo}
        <label class="fmt" class:on={chosen[f.id] && !disabled} class:disabled>
          <input type="checkbox" bind:checked={chosen[f.id]} disabled={disabled || running} />
          <span class="name">{f.label}</span>
          <span class="note">{disabled ? 'No screen video in this recording' : f.note}</span>
          {#if progress[f.id] !== undefined}
            <span class="bar"><span style:width="{Math.round(progress[f.id] * 100)}%"></span></span>
          {/if}
        </label>
      {/each}
    </div>

    <div class="opts">
      <label class="check">
        <input type="checkbox" bind:checked={fullFrame} disabled={running} />
        Show the whole screen in screenshots (instead of the clicked window)
      </label>
      {#if selected.some((f) => f === 'mp4' || f === 'mkv')}
        <label class="check" title={voiceReady ? 'Read each step aloud in the video, replacing your recorded narration' : 'Install a voice engine in Settings → Models first'}>
          <input type="checkbox" bind:checked={voiceover} disabled={running || !voiceReady} />
          Voiceover: read the steps aloud in the video{voiceReady ? '' : ' (needs a voice in Settings → Models)'}
        </label>
      {/if}
      {#if selected.some((f) => f === 'mp4' || f === 'mkv')}
        <p class="note-line">
          {clipCount > 0
            ? `Video: ${clipCount} step clip${clipCount > 1 ? 's' : ''} stitched in step order.`
            : 'Video: the whole recording (set clips on steps to trim it).'}
          Quality {settings.quality} ({qualityLabel(settings.quality)}), {settings.exportWidth
            ? `up to ${settings.exportWidth} px wide`
            : 'original size'}; change these in Settings.
        </p>
      {/if}
    </div>

    <div class="out">
      <span class="label">Save to</span>
      <div class="pick">
        <input bind:value={outDir} disabled={running} />
        <button onclick={browse} disabled={running}>Browse…</button>
      </div>
    </div>

    {#if error}<p class="error">{error}</p>{/if}

    <div class="actions">
      {#if done}
        <button onclick={() => revealItemInDir(done!.files[0] ?? done!.out_dir)}>Show in folder</button>
        {#if done.files.find((f) => f.endsWith('.pdf') || f.endsWith('.html'))}
          <button onclick={() => openPath(done!.files.find((f) => f.endsWith('.pdf') || f.endsWith('.html'))!)}>Open guide</button>
        {/if}
        <button class="primary" onclick={onclose}>Done</button>
      {:else}
        <button class="ghost" onclick={onclose} disabled={running}>Cancel</button>
        <button class="primary" onclick={run} disabled={running || selected.length === 0 || !outDir}>
          {running ? 'Exporting…' : `Export ${selected.length} format${selected.length === 1 ? '' : 's'}`}
        </button>
      {/if}
    </div>
  </div>
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    background: rgba(10, 10, 12, 0.45);
    display: grid;
    place-items: center;
    z-index: 40;
    padding: 24px;
  }
  .dialog {
    width: min(560px, 100%);
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: 16px;
    box-shadow: var(--shadow);
    padding: 24px;
    display: grid;
    gap: 18px;
  }
  h2 {
    margin: 0;
    font-size: 20px;
    letter-spacing: -0.02em;
  }
  .formats {
    display: grid;
    gap: 6px;
  }
  .fmt {
    display: grid;
    grid-template-columns: 20px 90px 1fr;
    align-items: center;
    gap: 10px;
    padding: 10px 12px;
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
    position: relative;
    overflow: hidden;
  }
  .fmt.on {
    border-color: color-mix(in srgb, var(--accent) 45%, transparent);
    background: var(--accent-soft);
  }
  .fmt.disabled {
    opacity: 0.5;
  }
  .fmt input,
  .check input {
    accent-color: var(--accent);
  }
  .name {
    font-weight: 600;
  }
  .note {
    color: var(--muted);
    font-size: 12.5px;
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
    transition: width 0.2s;
  }
  .opts {
    display: grid;
    gap: 6px;
  }
  .check {
    display: flex;
    gap: 8px;
    align-items: center;
    font-size: 13px;
  }
  .note-line {
    margin: 0;
    color: var(--muted);
    font-size: 12.5px;
  }
  .out {
    display: grid;
    gap: 6px;
  }
  .pick {
    display: flex;
    gap: 6px;
  }
  .pick input {
    flex: 1;
    min-width: 0;
    font-size: 13px;
  }
  .error {
    color: var(--danger);
    margin: 0;
    font-size: 13px;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
</style>

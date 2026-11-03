<script lang="ts">
  import { listen } from '@tauri-apps/api/event'
  import { onMount } from 'svelte'
  import { api, type Manifest, type ProcessProgress, type ProcessReport, type ProjectView, type Rect } from './api'
  import { settings } from './settings.svelte'
  import ExportDialog from './ExportDialog.svelte'
  import Inspector from './Inspector.svelte'
  import StepCanvas from './StepCanvas.svelte'
  import StepList from './StepList.svelte'

  let { project = $bindable(), onclose, onsettings }: {
    project: ProjectView
    onclose: () => void
    onsettings: () => void
  } = $props()

  const DEFAULT_HIGHLIGHT = '#ff5c35'
  let highlight = $derived(project.manifest.accent ?? DEFAULT_HIGHLIGHT)

  // Per-step data from the backend is indexed by the original order; key it
  // by step id so it survives reordering.
  const cropById = new Map<number, Rect | null>()
  const sizeById = new Map<number, [number, number] | null>()
  project.manifest.steps.forEach((s, i) => {
    cropById.set(s.id, project.default_crops[i] ?? null)
    sizeById.set(s.id, project.frame_sizes[i] ?? null)
  })

  let selectedId = $state<number | null>(project.manifest.steps[0]?.id ?? null)
  let savedJson = $state(JSON.stringify(project.manifest))
  let dirty = $derived(JSON.stringify(project.manifest) !== savedJson)
  let saving = $state(false)
  let saveError = $state<string | null>(null)
  let exporting = $state(false)

  let index = $derived(project.manifest.steps.findIndex((s) => s.id === selectedId))
  let step = $derived(index >= 0 ? project.manifest.steps[index] : null)

  async function save() {
    saving = true
    saveError = null
    try {
      const snapshot = $state.snapshot(project.manifest)
      await api.saveProject(snapshot)
      savedJson = JSON.stringify(snapshot)
    } catch (e) {
      saveError = String(e)
    } finally {
      saving = false
    }
  }

  function reorder(from: number, to: number) {
    const steps = project.manifest.steps
    const [moved] = steps.splice(from, 1)
    steps.splice(to, 0, moved)
  }

  function remove(id: number) {
    const steps = project.manifest.steps
    const i = steps.findIndex((s) => s.id === id)
    if (i < 0) return
    steps.splice(i, 1)
    if (selectedId === id) selectedId = steps[Math.min(i, steps.length - 1)]?.id ?? null
  }

  function close() {
    if (dirty && !confirm('You have unsaved changes. Close without saving?')) return
    onclose()
  }

  // Process: narration + on-screen text → draft step text.
  let processing = $state<ProcessProgress | null>(null)
  let processResult = $state<{ report: ProcessReport } | { error: string } | null>(null)

  async function runProcess() {
    processResult = null
    processing = { stage: 'start', fraction: 0, message: 'Starting' }
    try {
      await api.processProject($state.snapshot(project.manifest), settings.whisperModel)
    } catch (e) {
      processing = null
      processResult = { error: String(e) }
    }
  }

  function summary(r: ProcessReport): string {
    const parts = []
    if (r.labels) parts.push(`read ${r.labels} on-screen label${r.labels > 1 ? 's' : ''}`)
    if (r.tooltips) parts.push(`${r.tooltips} tooltip${r.tooltips > 1 ? 's' : ''}`)
    if (r.steps_with_narration) parts.push(`narration on ${r.steps_with_narration} step${r.steps_with_narration > 1 ? 's' : ''}`)
    const what = parts.length ? parts.join(', ') : 'found nothing new'
    return `Processed: ${what}. ${r.redrafted} step${r.redrafted === 1 ? '' : 's'} redrafted; edited text was left alone.`
  }

  onMount(() => {
    const un = [
      listen<ProcessProgress>('process-progress', (e) => (processing = e.payload)),
      listen<{ manifest: Manifest; report: ProcessReport }>('process-done', (e) => {
        processing = null
        project.manifest = e.payload.manifest
        processResult = { report: e.payload.report }
      }),
      listen<string>('process-failed', (e) => {
        processing = null
        processResult = { error: e.payload }
      }),
    ]
    return () => un.forEach((p) => p.then((f) => f()))
  })

  onMount(() => {
    const onkey = (e: KeyboardEvent) => {
      const typing = e.target instanceof HTMLTextAreaElement || e.target instanceof HTMLInputElement
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 's') {
        e.preventDefault()
        save()
      } else if (!typing && (e.key === 'ArrowDown' || e.key === 'ArrowUp')) {
        e.preventDefault()
        const next = index + (e.key === 'ArrowDown' ? 1 : -1)
        const s = project.manifest.steps[next]
        if (s) selectedId = s.id
      }
    }
    window.addEventListener('keydown', onkey)
    return () => window.removeEventListener('keydown', onkey)
  })
</script>

<div class="editor">
  <header>
    <button class="ghost back" onclick={close} title="Back to recordings">←</button>
    <div class="brand"><span class="dot"></span></div>
    <input
      class="title"
      placeholder="Untitled guide"
      value={project.manifest.title ?? ''}
      oninput={(e) => (project.manifest.title = e.currentTarget.value || undefined)}
    />
    <span class="status" class:dirty>
      {#if saving}Saving…{:else if saveError}Save failed{:else if dirty}Unsaved changes{:else}Saved{/if}
    </span>
    <label class="highlight" title="Highlight colour for markers, shapes and step numbers in this guide and its exports">
      <span class="chip" style:background={highlight}></span>
      Highlight
      <input
        type="color"
        value={highlight}
        oninput={(e) => {
          const v = e.currentTarget.value
          project.manifest.accent = v === DEFAULT_HIGHLIGHT ? undefined : v
        }}
      />
    </label>
    <button class="ghost" onclick={onsettings}>Settings</button>
    <button
      onclick={runProcess}
      disabled={processing !== null || project.manifest.steps.length === 0}
      title="Transcribe narration and read on-screen labels, then redraft step text you haven't edited"
    >
      ✦ Process
    </button>
    <button onclick={save} disabled={!dirty || saving} title="Save (Ctrl+S)">Save</button>
    <button class="primary" onclick={() => (exporting = true)} disabled={project.manifest.steps.length === 0}>Export</button>
  </header>

  {#if processing}
    <div class="banner">
      <span class="spinner"></span>
      <span>{processing.message}…</span>
      <span class="bar"><span style:width="{Math.round(processing.fraction * 100)}%"></span></span>
    </div>
  {:else if processResult}
    <div class="banner" class:error={'error' in processResult}>
      {#if 'error' in processResult}
        <span>Processing failed: {processResult.error}</span>
      {:else}
        <span>{summary(processResult.report)}</span>
        {#each processResult.report.skipped as s}
          <span class="skip">{s}. <button class="link" onclick={onsettings}>Get models</button></span>
        {/each}
      {/if}
      <button class="ghost dismiss" onclick={() => (processResult = null)}>✕</button>
    </div>
  {/if}

  {#if project.manifest.warnings?.length}
    <div class="warnings">
      {#each project.manifest.warnings as w}<div>⚠ {w}</div>{/each}
    </div>
  {/if}

  <div class="body">
    <StepList
      steps={project.manifest.steps}
      workDir={project.work_dir}
      {selectedId}
      onselect={(id) => (selectedId = id)}
      onreorder={reorder}
      ondelete={remove}
    />

    <section class="stage">
      {#if step}
        {#key step.id}
          <StepCanvas
            bind:step={project.manifest.steps[index]}
            workDir={project.work_dir}
            size={sizeById.get(step.id) ?? null}
            defaultCrop={cropById.get(step.id) ?? null}
            {highlight}
          />
        {/key}
      {:else}
        <div class="empty">This recording has no steps. Record again, or add steps in a future version.</div>
      {/if}
    </section>

    {#if step}
      <Inspector
        bind:step={project.manifest.steps[index]}
        number={index + 1}
        previewVideo={project.preview_video}
        duration={project.manifest.duration_ms}
        nextT={project.manifest.steps[index + 1]?.t_ms ?? null}
        ondelete={() => remove(step.id)}
      />
    {/if}
  </div>
</div>

{#if exporting}
  <ExportDialog
    manifest={project.manifest}
    hasVideo={!!project.manifest.video}
    onclose={() => (exporting = false)}
    onsaved={(json) => (savedJson = json)}
  />
{/if}

<style>
  .editor {
    height: 100%;
    display: flex;
    flex-direction: column;
  }
  header {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 16px;
    border-bottom: 1px solid var(--line);
    background: var(--panel);
  }
  .back {
    font-size: 16px;
    padding: 6px 10px;
  }
  .dot {
    display: block;
    width: 14px;
    height: 14px;
    border-radius: 50%;
    border: 3px solid var(--accent);
  }
  .title {
    flex: 1;
    min-width: 0;
    font-size: 16px;
    font-weight: 600;
    border-color: transparent;
    background: transparent;
    letter-spacing: -0.01em;
  }
  .title:hover {
    border-color: var(--line);
  }
  .highlight {
    position: relative;
    display: inline-flex;
    align-items: center;
    gap: 7px;
    font-size: 13px;
    color: var(--ink-2);
    padding: 6px 10px;
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  .highlight:hover {
    background: var(--panel-2);
  }
  .highlight .chip {
    width: 14px;
    height: 14px;
    border-radius: 50%;
    box-shadow: 0 0 0 1px var(--line-strong);
  }
  .highlight input {
    position: absolute;
    inset: 0;
    opacity: 0;
    cursor: pointer;
  }
  .status {
    color: var(--muted);
    font-size: 12.5px;
    margin-right: 4px;
  }
  .status.dirty {
    color: var(--accent);
  }
  .banner {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px 14px;
    padding: 8px 16px;
    border-bottom: 1px solid var(--line);
    background: var(--accent-soft);
    font-size: 13px;
  }
  .banner.error {
    background: color-mix(in srgb, var(--danger) 12%, var(--panel));
  }
  .banner .bar {
    flex: 0 0 160px;
    height: 4px;
    border-radius: 2px;
    background: var(--line);
    overflow: hidden;
  }
  .banner .bar span {
    display: block;
    height: 100%;
    background: var(--accent);
    transition: width 0.2s;
  }
  .banner .skip {
    color: var(--ink-2);
  }
  .banner .link {
    border: none;
    background: none;
    padding: 0;
    color: var(--accent);
    text-decoration: underline;
  }
  .banner .dismiss {
    margin-left: auto;
    padding: 2px 8px;
  }
  .spinner {
    width: 12px;
    height: 12px;
    border-radius: 50%;
    border: 2px solid var(--line-strong);
    border-top-color: var(--accent);
    animation: spin 0.8s linear infinite;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
  .warnings {
    background: var(--accent-soft);
    border-bottom: 1px solid var(--line);
    padding: 8px 16px;
    font-size: 13px;
  }
  .body {
    flex: 1;
    display: grid;
    grid-template-columns: 290px 1fr 340px;
    min-height: 0;
  }
  .stage {
    min-width: 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
    background: var(--bg);
  }
  .empty {
    margin: auto;
    color: var(--muted);
    max-width: 320px;
    text-align: center;
  }
</style>

<script lang="ts">
  import { onMount } from 'svelte'
  import { api, type ProjectView, type Rect } from './api'
  import ExportDialog from './ExportDialog.svelte'
  import Inspector from './Inspector.svelte'
  import StepCanvas from './StepCanvas.svelte'
  import StepList from './StepList.svelte'

  let { project = $bindable(), onclose }: { project: ProjectView; onclose: () => void } = $props()

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
    <button onclick={save} disabled={!dirty || saving} title="Save (Ctrl+S)">Save</button>
    <button class="primary" onclick={() => (exporting = true)} disabled={project.manifest.steps.length === 0}>Export</button>
  </header>

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
  .status {
    color: var(--muted);
    font-size: 12.5px;
    margin-right: 4px;
  }
  .status.dirty {
    color: var(--accent);
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

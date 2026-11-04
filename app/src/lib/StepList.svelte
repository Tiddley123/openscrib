<script lang="ts">
  import { fileUrl, formatTime, stepNumbers, type Step } from './api'

  let { steps, workDir, selectedId, onselect, onreorder, ondelete, onrestore, onadd }: {
    steps: Step[]
    workDir: string
    selectedId: number | null
    onselect: (id: number) => void
    onreorder: (from: number, to: number) => void
    ondelete: (id: number) => void
    onrestore: (id: number) => void
    /** Add a step after the selected one, optionally from an image file. */
    onadd: (withImage: boolean) => void
  } = $props()

  let dragFrom = $state<number | null>(null)
  let dropAt = $state<number | null>(null)
  let numbers = $derived(stepNumbers(steps))

  function plain(md: string): string {
    return md.replace(/\*\*|`|_/g, '')
  }

  function drop(to: number) {
    if (dragFrom !== null && dragFrom !== to) {
      onreorder(dragFrom, to > dragFrom ? to - 1 : to)
    }
    dragFrom = null
    dropAt = null
  }
</script>

<aside>
  <div class="head">
    <span class="label">Steps <span class="count">{numbers.size}</span></span>
    <span class="add">
      <button class="ghost small" onclick={() => onadd(false)} title="Add a text step after the selected step (for narration, notes or warnings)">+ Step</button>
      <button class="ghost small" onclick={() => onadd(true)} title="Add a step with an image from your computer">+ Image</button>
    </span>
  </div>
  <ol>
    {#each steps as s, i (s.id)}
      <li
        class:drop-before={dropAt === i && dragFrom !== null && dragFrom !== i && dragFrom !== i - 1}
        ondragover={(e) => {
          e.preventDefault()
          const r = e.currentTarget.getBoundingClientRect()
          dropAt = e.clientY < r.top + r.height / 2 ? i : i + 1
        }}
        ondrop={(e) => {
          e.preventDefault()
          drop(dropAt ?? i)
        }}
      >
        <button
          class="step"
          class:selected={s.id === selectedId}
          class:deleted={s.deleted}
          draggable="true"
          ondragstart={(e) => {
            dragFrom = i
            e.dataTransfer?.setData('text/plain', String(s.id))
          }}
          ondragend={() => {
            dragFrom = null
            dropAt = null
          }}
          onclick={() => onselect(s.id)}
        >
          <span class="n">{numbers.get(s.id) ?? '–'}</span>
          <span class="thumb">
            {#if s.frame}<img src={fileUrl(workDir, s.frame)} alt="" loading="lazy" draggable="false" />{/if}
          </span>
          <span class="text">
            <span class="line">{plain(s.text) || 'Untitled step'}</span>
            <span class="meta">{formatTime(s.t_ms)}{s.deleted ? ' · deleted' : ''}{s.clip ? ' · clip' : ''}{s.annotations.length ? ` · ${s.annotations.length} note${s.annotations.length > 1 ? 's' : ''}` : ''}</span>
          </span>
        </button>
        {#if s.deleted}
          <button class="ghost del" title="Restore step" onclick={() => onrestore(s.id)}>↺</button>
        {:else}
          <button class="ghost del" title="Delete step (it stays here, greyed out, so you can still set redactions on it)" onclick={() => ondelete(s.id)}>✕</button>
        {/if}
      </li>
    {/each}
    <li
      class="tail"
      class:drop-before={dropAt === steps.length && dragFrom !== null && dragFrom !== steps.length - 1}
      ondragover={(e) => {
        e.preventDefault()
        dropAt = steps.length
      }}
      ondrop={(e) => {
        e.preventDefault()
        drop(steps.length)
      }}
    ></li>
  </ol>
</aside>

<style>
  aside {
    border-right: 1px solid var(--line);
    background: var(--panel);
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .head {
    display: flex;
    justify-content: space-between;
    padding: 14px 16px 8px;
  }
  .head {
    align-items: center;
  }
  .add {
    display: flex;
    gap: 2px;
  }
  .small {
    padding: 3px 8px;
    font-size: 12px;
  }
  .count {
    color: var(--muted);
    font-size: 12px;
  }
  ol {
    list-style: none;
    margin: 0;
    padding: 4px 8px 16px;
    overflow: auto;
    flex: 1;
  }
  li {
    position: relative;
    border-top: 2px solid transparent;
  }
  li.drop-before {
    border-top-color: var(--accent);
  }
  .tail {
    height: 24px;
  }
  .step {
    width: 100%;
    display: grid;
    grid-template-columns: 22px 64px 1fr;
    gap: 10px;
    align-items: center;
    text-align: left;
    padding: 8px;
    border: 1px solid transparent;
    border-radius: var(--radius-sm);
    background: transparent;
    margin: 2px 0;
  }
  .step:hover {
    background: var(--panel-2);
    border-color: var(--line);
  }
  .step.selected {
    background: var(--accent-soft);
    border-color: color-mix(in srgb, var(--accent) 35%, transparent);
  }
  .step.deleted .thumb,
  .step.deleted .n {
    filter: grayscale(1);
    opacity: 0.45;
  }
  .step.deleted .line {
    color: var(--muted);
    text-decoration: line-through;
  }
  .n {
    width: 22px;
    height: 22px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    background: var(--panel-2);
    border: 1px solid var(--line);
    font-size: 11px;
    font-weight: 700;
  }
  .selected .n {
    background: var(--accent);
    border-color: var(--accent);
    color: #fff;
  }
  .thumb {
    width: 64px;
    height: 40px;
    border-radius: 5px;
    overflow: hidden;
    background: var(--line);
  }
  .thumb img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
  }
  .text {
    min-width: 0;
    display: grid;
  }
  .line {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 13px;
  }
  .meta {
    color: var(--muted);
    font-size: 11.5px;
  }
  .del {
    position: absolute;
    right: 6px;
    top: 50%;
    transform: translateY(-50%);
    padding: 3px 7px;
    font-size: 11px;
    opacity: 0;
    color: var(--muted);
  }
  li:hover .del {
    opacity: 1;
  }
</style>

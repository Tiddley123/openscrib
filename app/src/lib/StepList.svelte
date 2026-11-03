<script lang="ts">
  import { fileUrl, formatTime, type Step } from './api'

  let { steps, workDir, selectedId, onselect, onreorder, ondelete }: {
    steps: Step[]
    workDir: string
    selectedId: number | null
    onselect: (id: number) => void
    onreorder: (from: number, to: number) => void
    ondelete: (id: number) => void
  } = $props()

  let dragFrom = $state<number | null>(null)
  let dropAt = $state<number | null>(null)

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
    <span class="label">Steps</span>
    <span class="count">{steps.length}</span>
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
          <span class="n">{i + 1}</span>
          <span class="thumb">
            {#if s.frame}<img src={fileUrl(workDir, s.frame)} alt="" loading="lazy" draggable="false" />{/if}
          </span>
          <span class="text">
            <span class="line">{plain(s.text) || 'Untitled step'}</span>
            <span class="meta">{formatTime(s.t_ms)}{s.clip ? ' · clip' : ''}{s.annotations.length ? ` · ${s.annotations.length} note${s.annotations.length > 1 ? 's' : ''}` : ''}</span>
          </span>
        </button>
        <button class="ghost del" title="Delete step" onclick={() => ondelete(s.id)}>✕</button>
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

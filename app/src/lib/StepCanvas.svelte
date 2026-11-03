<script lang="ts">
  import { onMount } from 'svelte'
  import { fileUrl, type Annotation, type Point, type Rect, type Step } from './api'

  type Tool = 'select' | 'marker' | 'arrow' | 'box' | 'blur' | 'crop'

  let { step = $bindable(), workDir, size, defaultCrop, highlight }: {
    step: Step
    workDir: string
    size: [number, number] | null
    defaultCrop: Rect | null
    /** Project highlight colour for markers and shapes. */
    highlight: string
  } = $props()

  const tools: { id: Tool; label: string; key: string; hint: string }[] = [
    { id: 'select', label: 'Select', key: 'V', hint: 'Click a note to select it, then Delete. Right-click any note to delete it.' },
    { id: 'arrow', label: 'Arrow', key: 'A', hint: 'Drag from tail to tip' },
    { id: 'box', label: 'Box', key: 'B', hint: 'Drag to outline an area' },
    { id: 'marker', label: 'Marker', key: 'M', hint: 'Click to add a ring' },
    { id: 'blur', label: 'Redact', key: 'R', hint: 'Drag over anything private; exports paint it out solid' },
    { id: 'crop', label: 'Crop', key: 'C', hint: 'Drag the region exports should show' },
  ]

  let tool = $state<Tool>('select')
  // The backend knows the frame size; fall back to the image's own.
  // svelte-ignore state_referenced_locally
  let natural = $state<[number, number] | null>(size)
  let box = $state({ w: 0, h: 0 })
  let container: HTMLDivElement
  let selected = $state<number | null>(null)
  let draft = $state<{ from: Point; to: Point } | null>(null)
  let showPre = $state(false)

  let W = $derived(natural?.[0] ?? 1)
  let H = $derived(natural?.[1] ?? 1)
  let crop = $derived(step.crop ?? defaultCrop)
  let src = $derived(step.frame ? fileUrl(workDir, showPre && step.pre_frame ? step.pre_frame : step.frame) : null)

  function fit() {
    if (!container || !natural) return
    const pad = 32
    const cw = container.clientWidth - pad * 2
    const ch = container.clientHeight - pad * 2
    const s = Math.min(cw / W, ch / H)
    box = { w: Math.max(0, W * s), h: Math.max(0, H * s) }
  }

  onMount(() => {
    const ro = new ResizeObserver(fit)
    ro.observe(container)
    const onkey = (e: KeyboardEvent) => {
      if (e.target instanceof HTMLTextAreaElement || e.target instanceof HTMLInputElement) return
      if ((e.key === 'Delete' || e.key === 'Backspace') && selected !== null) {
        step.annotations.splice(selected, 1)
        selected = null
        e.preventDefault()
        return
      }
      if (e.ctrlKey || e.metaKey || e.altKey) return
      const t = tools.find((t) => t.key.toLowerCase() === e.key.toLowerCase())
      if (t) tool = t.id
      if (e.key === 'Escape') {
        draft = null
        selected = null
      }
    }
    window.addEventListener('keydown', onkey)
    return () => {
      ro.disconnect()
      window.removeEventListener('keydown', onkey)
    }
  })

  $effect(() => {
    natural
    fit()
  })

  function toFrame(e: PointerEvent): Point {
    const r = (e.currentTarget as Element).getBoundingClientRect()
    return {
      x: Math.round(((e.clientX - r.left) / r.width) * W),
      y: Math.round(((e.clientY - r.top) / r.height) * H),
    }
  }

  function rectOf(a: Point, b: Point): Rect {
    const x = Math.max(0, Math.min(a.x, b.x))
    const y = Math.max(0, Math.min(a.y, b.y))
    return { x, y, w: Math.min(W, Math.max(a.x, b.x)) - x, h: Math.min(H, Math.max(a.y, b.y)) - y }
  }

  function down(e: PointerEvent) {
    if (tool === 'select') {
      selected = null
      return
    }
    const p = toFrame(e)
    if (tool === 'marker') {
      step.annotations.push({ type: 'marker', at: p })
      return
    }
    ;(e.currentTarget as Element).setPointerCapture(e.pointerId)
    draft = { from: p, to: p }
  }

  function move(e: PointerEvent) {
    if (draft) draft.to = toFrame(e)
  }

  function up() {
    if (!draft) return
    const { from, to } = draft
    draft = null
    const big = Math.abs(to.x - from.x) > 6 || Math.abs(to.y - from.y) > 6
    if (!big) return
    if (tool === 'arrow') step.annotations.push({ type: 'arrow', from, to })
    else if (tool === 'box') step.annotations.push({ type: 'box', rect: rectOf(from, to) })
    else if (tool === 'blur') step.annotations.push({ type: 'blur', rect: rectOf(from, to) })
    else if (tool === 'crop') step.crop = rectOf(from, to)
  }

  function removeNote(i: number) {
    step.annotations.splice(i, 1)
    if (selected === i) selected = null
    else if (selected !== null && selected > i) selected -= 1
  }

  function pick(e: PointerEvent, i: number) {
    if (tool !== 'select') return
    e.stopPropagation()
    selected = i
  }

  // Overlay sizes are in frame pixels; scale them so they read the same at
  // any zoom.
  let k = $derived(box.w ? W / box.w : 1)

  function head(a: Point, b: Point): string {
    const len = Math.hypot(b.x - a.x, b.y - a.y) || 1
    const ux = (b.x - a.x) / len
    const uy = (b.y - a.y) / len
    const s = Math.min(18 * k, len * 0.6)
    const pts = [0.5, -0.5].map((ang) => {
      const c = Math.cos(ang)
      const sn = Math.sin(ang)
      return `${b.x - (ux * c - uy * sn) * s},${b.y - (ux * sn + uy * c) * s}`
    })
    return `${pts[0]} ${b.x},${b.y} ${pts[1]}`
  }
</script>

<div class="toolbar">
  <div class="tools" role="toolbar">
    {#each tools as t}
      <button class:active={tool === t.id} onclick={() => (tool = t.id)} title={`${t.hint} (${t.key})`}>
        {t.label}
      </button>
    {/each}
  </div>
  <div class="right">
    {#if step.pre_frame}
      <label class="pre" title="The frame captured when the mouse rested before the click, which often shows the tooltip">
        <input type="checkbox" bind:checked={showPre} /> Before click
      </label>
    {/if}
    {#if selected !== null}
      <button class="danger" onclick={() => selected !== null && removeNote(selected)}>Delete note</button>
    {/if}
    {#if step.crop}
      <button class="ghost" onclick={() => (step.crop = undefined)}>Reset crop</button>
    {/if}
  </div>
</div>
<p class="hint">{tools.find((t) => t.id === tool)?.hint}</p>

<div class="canvas" bind:this={container}>
  {#if src}
    <div class="frame" style:width="{box.w}px" style:height="{box.h}px">
      <img
        {src}
        alt="Step frame"
        draggable="false"
        onload={(e) => {
          const img = e.currentTarget as HTMLImageElement
          if (!natural) natural = [img.naturalWidth, img.naturalHeight]
        }}
      />
      <svg
        viewBox="0 0 {W} {H}"
        class="tool-{tool}"
        onpointerdown={down}
        onpointermove={move}
        onpointerup={up}
        role="application"
      >
        <defs>
          <pattern id="hatch" width={12 * k} height={12 * k} patternUnits="userSpaceOnUse" patternTransform="rotate(45)">
            <rect width={12 * k} height={12 * k} fill="#202024" />
            <line x1="0" y1="0" x2="0" y2={12 * k} stroke="#2c2c31" stroke-width={5 * k} />
          </pattern>
        </defs>

        {#if crop}
          <path
            d="M0 0H{W}V{H}H0Z M{crop.x} {crop.y}v{crop.h}h{crop.w}v{-crop.h}Z"
            fill="rgba(10,10,12,0.5)"
            fill-rule="evenodd"
            pointer-events="none"
          />
          <rect
            x={crop.x}
            y={crop.y}
            width={crop.w}
            height={crop.h}
            fill="none"
            stroke={step.crop ? '#fff' : 'rgba(255,255,255,0.7)'}
            stroke-dasharray={step.crop ? 'none' : `${8 * k} ${6 * k}`}
            stroke-width={1.5 * k}
            pointer-events="none"
          />
        {/if}

        {#each step.annotations as a, i}
          <g
            class="ann"
            class:sel={selected === i}
            role="button"
            tabindex="-1"
            aria-label="{a.type} note"
            onpointerdown={(e) => pick(e, i)}
            oncontextmenu={(e) => {
              e.preventDefault()
              removeNote(i)
            }}
          >
            {#if a.type === 'blur'}
              <rect x={a.rect.x} y={a.rect.y} width={a.rect.w} height={a.rect.h} fill="url(#hatch)" />
            {:else if a.type === 'box'}
              <rect x={a.rect.x} y={a.rect.y} width={a.rect.w} height={a.rect.h} fill="none" stroke={highlight} stroke-width={4 * k} />
            {:else if a.type === 'arrow'}
              <line x1={a.from.x} y1={a.from.y} x2={a.to.x} y2={a.to.y} stroke={highlight} stroke-width={5 * k} stroke-linecap="round" />
              <polyline points={head(a.from, a.to)} fill="none" stroke={highlight} stroke-width={5 * k} stroke-linecap="round" stroke-linejoin="round" />
            {:else if a.type === 'marker'}
              <circle cx={a.at.x} cy={a.at.y} r={30} fill="none" stroke="#fff" stroke-width={7} opacity="0.9" />
              <circle cx={a.at.x} cy={a.at.y} r={30} fill="none" stroke={highlight} stroke-width={4} />
            {/if}
          </g>
        {/each}

        {#if step.click}
          <circle cx={step.click.in_frame.x} cy={step.click.in_frame.y} r={30} fill="none" stroke="#fff" stroke-width={7} opacity="0.9" pointer-events="none" />
          <circle
            cx={step.click.in_frame.x}
            cy={step.click.in_frame.y}
            r={30}
            fill="none"
            stroke={step.click.button === 'right' ? '#3584ff' : highlight}
            stroke-width={4}
            pointer-events="none"
          />
        {/if}

        {#if draft}
          {@const r = rectOf(draft.from, draft.to)}
          {#if tool === 'arrow'}
            <line x1={draft.from.x} y1={draft.from.y} x2={draft.to.x} y2={draft.to.y} stroke={highlight} stroke-width={5 * k} stroke-linecap="round" />
          {:else}
            <rect
              x={r.x}
              y={r.y}
              width={r.w}
              height={r.h}
              fill={tool === 'blur' ? 'url(#hatch)' : 'none'}
              stroke={tool === 'crop' ? '#fff' : highlight}
              stroke-width={2 * k}
              stroke-dasharray="{6 * k} {4 * k}"
            />
          {/if}
        {/if}
      </svg>
    </div>
  {:else}
    <div class="noframe">This step has no screenshot.</div>
  {/if}
</div>

<style>
  .toolbar {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 12px 16px 0;
    gap: 12px;
  }
  .tools {
    display: inline-flex;
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: 10px;
    padding: 3px;
    gap: 2px;
  }
  .tools button {
    border: none;
    background: transparent;
    padding: 6px 12px;
    border-radius: 7px;
    font-size: 13px;
  }
  .tools button.active {
    background: var(--ink);
    color: var(--bg);
  }
  .right {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .pre {
    display: flex;
    gap: 6px;
    align-items: center;
    font-size: 13px;
    color: var(--ink-2);
  }
  .pre input {
    accent-color: var(--accent);
  }
  .hint {
    margin: 6px 16px 0;
    font-size: 12px;
    color: var(--muted);
    min-height: 18px;
  }
  .canvas {
    flex: 1;
    min-height: 0;
    display: grid;
    place-items: center;
    overflow: hidden;
  }
  .frame {
    position: relative;
    box-shadow: var(--shadow);
    border-radius: 6px;
    overflow: hidden;
    background: #000;
  }
  .frame img {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    display: block;
    user-select: none;
  }
  svg {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    touch-action: none;
  }
  svg:not(.tool-select) {
    cursor: crosshair;
  }
  .ann {
    cursor: default;
  }
  .tool-select .ann {
    cursor: pointer;
  }
  .ann.sel {
    filter: drop-shadow(0 0 6px var(--accent)) drop-shadow(0 0 2px #fff);
  }
  .noframe {
    color: var(--muted);
  }
</style>

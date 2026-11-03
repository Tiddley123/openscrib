<script lang="ts">
  import { api, formatTime, localUrl, syncTranscripts, type Callout, type Manifest, type NarrationSegment, type Step } from './api'

  let { step = $bindable(), manifest = $bindable(), number, previewVideo, duration, nextT, ondelete }: {
    step: Step
    manifest: Manifest
    number: number
    previewVideo: string | null
    duration: number
    nextT: number | null
    ondelete: () => void
  } = $props()

  let video = $state<HTMLVideoElement | null>(null)
  let now = $state(0)
  let playingClip = $state(false)

  const LEAD_MS = 1500

  // Jump the preview to this step whenever a different step is shown.
  $effect(() => {
    const target = (step.clip?.start_ms ?? Math.max(0, step.t_ms - LEAD_MS)) / 1000
    if (video && video.readyState >= 1) video.currentTime = target
  })

  function seekOnLoad() {
    if (video) video.currentTime = (step.clip?.start_ms ?? Math.max(0, step.t_ms - LEAD_MS)) / 1000
  }

  function ms(): number {
    return Math.round((video?.currentTime ?? 0) * 1000)
  }

  function setStart() {
    const s = ms()
    const end = step.clip && step.clip.end_ms > s ? step.clip.end_ms : Math.min(duration, s + 4000)
    step.clip = { start_ms: s, end_ms: end }
  }

  function setEnd() {
    const e = ms()
    const start = step.clip && step.clip.start_ms < e ? step.clip.start_ms : Math.max(0, e - 4000)
    step.clip = { start_ms: start, end_ms: e }
  }

  function auto() {
    const start = Math.max(0, step.t_ms - LEAD_MS)
    const end = Math.min(duration, nextT !== null ? Math.max(nextT - 300, step.t_ms + 1500) : step.t_ms + 4000)
    step.clip = { start_ms: start, end_ms: end }
  }

  function playClip() {
    if (!video || !step.clip) return
    video.currentTime = step.clip.start_ms / 1000
    playingClip = true
    video.play()
  }

  function tick() {
    now = ms()
    if (playingClip && step.clip && video && now >= step.clip.end_ms) {
      video.pause()
      playingClip = false
    }
  }

  import { clearCrop, removeNote } from './carry'
  const here = () => manifest.steps.findIndex((s) => s.id === step.id)

  const kinds = { click: 'Click', typed: 'Typing', shortcut: 'Shortcut', note: 'Added step' }

  function useText(t: string) {
    step.text = t
    step.text_edited = true
  }

  function verb(): string {
    const c = step.click
    if (c?.double) return 'Double-click'
    if (c?.button === 'right') return 'Right-click'
    return 'Click'
  }

  function sentence(s: string): string {
    const t = s.trim().replace(/\s+/g, ' ')
    if (!t) return t
    const cap = t[0].toUpperCase() + t.slice(1)
    return /[.!?]$/.test(cap) ? cap : `${cap}.`
  }

  async function resetText() {
    step.text_edited = false
    step.text = await api.draftText($state.snapshot(step)).catch(() => step.text)
  }
  const noteLabels = { arrow: 'Arrow', box: 'Box', marker: 'Marker', blur: 'Redaction', sticker: 'Sticker' }

  const callouts: { id: Callout; label: string; glyph: string }[] = [
    { id: 'note', label: 'Note', glyph: 'i' },
    { id: 'tip', label: 'Tip', glyph: '★' },
    { id: 'warning', label: 'Warning', glyph: '!' },
    { id: 'important', label: 'Important', glyph: '!' },
    { id: 'quote', label: 'Quote', glyph: '“' },
  ]

  // Narration: every transcribed segment, attachable to any step.
  let mine = $derived((manifest.narration ?? []).filter((n) => n.step === step.id).length)
  const stepNumber = (id: number) => manifest.steps.findIndex((s) => s.id === id) + 1

  async function toggleNarration(n: NarrationSegment) {
    n.step = n.step === step.id ? undefined : step.id
    syncTranscripts(manifest)
    // Let untouched steps pick up their new narration straight away.
    for (const s of manifest.steps) {
      if (!s.text_edited) s.text = await api.draftText($state.snapshot(s)).catch(() => s.text)
    }
  }
</script>

<aside>
  <div class="section">
    <div class="row">
      <span class="label">Step {number}</span>
      <span class="chip">{kinds[step.kind]}</span>
    </div>
    <textarea
      rows="4"
      value={step.text}
      oninput={(e) => {
        step.text = e.currentTarget.value
        step.text_edited = true
      }}
      placeholder="Describe this step"
    ></textarea>
    <p class="hint">Use <code>**bold**</code> for UI labels and <code>`code`</code> for typed values.</p>
    {#if step.text_edited}
      <button class="ghost small reset" onclick={resetText} title="Replace with the automatic text from the narration and on-screen labels">
        Reset to automatic text
      </button>
    {/if}
    <div class="callouts" role="group" aria-label="Callout style">
      {#each callouts as c}
        <button
          class="callout {c.id}"
          class:on={step.callout === c.id}
          title="Show this step as a {c.label.toLowerCase()} box with a big icon in exported guides"
          onclick={() => (step.callout = step.callout === c.id ? undefined : c.id)}
        >
          <span class="glyph">{c.glyph}</span>{c.label}
        </button>
      {/each}
    </div>
  </div>

  {#if manifest.narration?.length}
    <div class="section">
      <div class="row">
        <span class="label">Narration</span>
        <span class="count">{mine} on this step</span>
      </div>
      <ul class="narr">
        {#each manifest.narration as n}
          {@const owner = n.step !== undefined && n.step !== step.id ? stepNumber(n.step) || null : null}
          <li class:mine={n.step === step.id}>
            <span class="time">{formatTime(n.start_ms)}</span>
            <span class="said">{n.text}</span>
            <button class="ghost small" onclick={() => toggleNarration(n)} title={n.step === step.id ? 'Detach from this step' : 'Attach to this step'}>
              {n.step === step.id ? 'Detach' : owner ? `Move from ${owner}` : 'Attach'}
            </button>
          </li>
        {/each}
      </ul>
      <p class="hint">Everything said while recording. Attach a line to this step to use it in the step text; all of it becomes video captions.</p>
    </div>
  {/if}

  {#if step.transcript || step.ocr || step.tooltip}
    <div class="section">
      <div class="label">Context</div>
      <dl class="context">
        {#if step.transcript}
          <dt>You said</dt>
          <dd>
            <span>“{step.transcript}”</span>
            <button class="ghost small" onclick={() => useText(sentence(step.transcript!))}>Use</button>
          </dd>
        {/if}
        {#if step.ocr}
          <dt>Clicked on</dt>
          <dd>
            <span><b>{step.ocr}</b></span>
            <button class="ghost small" onclick={() => useText(`${verb()} **${step.ocr}**.`)}>Use</button>
          </dd>
        {/if}
        {#if step.tooltip}
          <dt>Tooltip</dt>
          <dd>
            <span>{step.tooltip}</span>
            <button class="ghost small" onclick={() => useText(`${verb()} **${step.tooltip}**.`)}>Use</button>
          </dd>
        {/if}
      </dl>
    </div>
  {/if}

  <div class="section">
    <div class="row">
      <span class="label">Notes</span>
      {#if step.annotations.length > 1}
        <button class="ghost small" onclick={() => { for (let i = step.annotations.length - 1; i >= 0; i--) removeNote(manifest, here(), i) }}>Remove all</button>
      {/if}
    </div>
    {#if step.annotations.length === 0 && !step.crop}
      <p class="hint">Draw arrows, boxes, markers and redactions with the tools above the image.</p>
    {:else}
      <ul class="notes">
        {#each step.annotations as a, i}
          <li>
            <span class="swatch {a.type}"></span>
            <span>{noteLabels[a.type]}</span>
            <button class="ghost small" title="Delete this note (a carried redaction also goes from later steps)" onclick={() => removeNote(manifest, here(), i)}>✕</button>
          </li>
        {/each}
        {#if step.crop}
          <li>
            <span class="swatch crop"></span>
            <span>Custom crop</span>
            <button class="ghost small" title="Go back to the default crop (here and on later steps it was carried to)" onclick={() => clearCrop(manifest, here())}>✕</button>
          </li>
        {/if}
      </ul>
    {/if}
  </div>

  <div class="section">
    <div class="label">Video clip</div>
    {#if previewVideo}
      <!-- svelte-ignore a11y_media_has_caption -->
      <video
        bind:this={video}
        src={localUrl(previewVideo)}
        controls
        preload="metadata"
        onloadedmetadata={seekOnLoad}
        ontimeupdate={tick}
        onpause={() => (playingClip = false)}
      ></video>
      <div class="clip">
        {#if step.clip}
          <span><b>{formatTime(step.clip.start_ms)}</b> → <b>{formatTime(step.clip.end_ms)}</b>
            <span class="muted">({((step.clip.end_ms - step.clip.start_ms) / 1000).toFixed(1)}s)</span></span>
        {:else}
          <span class="muted">No clip. The video export uses the whole recording unless at least one step has a clip.</span>
        {/if}
      </div>
      <div class="buttons">
        <button onclick={setStart} title="Use the current video position as the clip start">Set start</button>
        <button onclick={setEnd} title="Use the current video position as the clip end">Set end</button>
        <button onclick={auto} title="From 1.5 s before the click to the next step">Auto</button>
        {#if step.clip}
          <button onclick={playClip}>▶ Play clip</button>
          <button class="ghost" onclick={() => (step.clip = undefined)}>Clear</button>
        {/if}
      </div>
      <p class="hint">At {formatTime(now)} · the step happens at {formatTime(step.t_ms)}</p>
    {:else}
      <p class="muted">This recording has no screen video.</p>
    {/if}
  </div>

  <div class="section details">
    <div class="label">Captured</div>
    <dl>
      <dt>Time</dt>
      <dd>{formatTime(step.t_ms)}</dd>
      {#if step.window}
        <dt>Window</dt>
        <dd title={step.window.title}>{step.window.title || '(untitled)'}</dd>
        <dt>App</dt>
        <dd>{step.window.process}</dd>
      {/if}
      {#if step.keys}
        <dt>Keys</dt>
        <dd><code>{step.keys}</code></dd>
      {/if}
      {#if step.click}
        <dt>Position</dt>
        <dd>{step.click.x}, {step.click.y}{step.click.double ? ' · double' : ''}</dd>
      {/if}
    </dl>
  </div>

  <div class="section">
    <button class="danger" onclick={ondelete}>Delete step</button>
  </div>
</aside>

<style>
  aside {
    border-left: 1px solid var(--line);
    background: var(--panel);
    overflow: auto;
    min-height: 0;
  }
  .section {
    padding: 16px;
    border-bottom: 1px solid var(--line);
    display: grid;
    gap: 10px;
  }
  .row {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }
  .chip {
    font-size: 11.5px;
    padding: 2px 8px;
    border-radius: 99px;
    background: var(--panel-2);
    border: 1px solid var(--line);
    color: var(--ink-2);
  }
  textarea {
    resize: vertical;
    font-size: 14.5px;
    line-height: 1.5;
    min-height: 90px;
  }
  .hint {
    margin: 0;
    color: var(--muted);
    font-size: 12px;
  }
  code {
    font: 12px ui-monospace, 'Cascadia Mono', Consolas, monospace;
  }
  video {
    width: 100%;
    border-radius: var(--radius-sm);
    background: #000;
    aspect-ratio: 16 / 9;
  }
  .clip {
    font-size: 13px;
  }
  .callouts {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
  }
  .callout {
    padding: 3px 9px 3px 4px;
    font-size: 12px;
    border-radius: 99px;
    gap: 5px;
    --c: #0a84ff;
  }
  .callout.tip { --c: #30a46c; }
  .callout.warning { --c: #f5a524; }
  .callout.important { --c: #e5484d; }
  .callout.quote { --c: var(--accent); }
  .callout .glyph {
    width: 18px;
    height: 18px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    background: var(--c);
    color: #fff;
    font-weight: 800;
    font-size: 11px;
  }
  .callout.on {
    border-color: var(--c);
    background: color-mix(in srgb, var(--c) 14%, var(--panel));
  }
  .count {
    font-size: 12px;
    color: var(--muted);
  }
  .narr {
    list-style: none;
    margin: 0;
    padding: 0;
    max-height: 240px;
    overflow: auto;
    display: grid;
    gap: 2px;
  }
  .narr li {
    display: grid;
    grid-template-columns: 44px 1fr auto;
    gap: 8px;
    align-items: baseline;
    padding: 5px 6px;
    border-radius: 6px;
    font-size: 13px;
  }
  .narr li.mine {
    background: var(--accent-soft);
  }
  .narr .time {
    color: var(--muted);
    font-size: 11.5px;
    font-variant-numeric: tabular-nums;
  }
  .reset {
    justify-self: start;
  }
  .context {
    grid-template-columns: 72px 1fr;
  }
  .context dd {
    display: flex;
    gap: 8px;
    align-items: baseline;
    justify-content: space-between;
    white-space: normal;
  }
  .context dd span {
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .notes {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 2px;
  }
  .notes li {
    display: grid;
    grid-template-columns: 14px 1fr auto;
    align-items: center;
    gap: 10px;
    padding: 4px 4px 4px 8px;
    border-radius: 6px;
    font-size: 13px;
  }
  .notes li:hover {
    background: var(--panel-2);
  }
  .swatch {
    width: 12px;
    height: 12px;
    border-radius: 3px;
    border: 2px solid var(--accent);
  }
  .swatch.marker {
    border-radius: 50%;
  }
  .swatch.arrow {
    border: none;
    height: 3px;
    background: var(--accent);
    transform: rotate(-30deg);
  }
  .swatch.blur {
    border: none;
    background: #202024;
  }
  .swatch.crop {
    border-color: var(--muted);
    border-style: dashed;
  }
  .small {
    padding: 3px 8px;
    font-size: 12px;
    color: var(--muted);
  }
  .muted {
    color: var(--muted);
  }
  .buttons {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .buttons button {
    padding: 5px 10px;
    font-size: 13px;
  }
  dl {
    display: grid;
    grid-template-columns: 70px 1fr;
    gap: 6px 10px;
    margin: 0;
    font-size: 13px;
  }
  dt {
    color: var(--muted);
  }
  dd {
    margin: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>

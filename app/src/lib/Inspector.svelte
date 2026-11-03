<script lang="ts">
  import { formatTime, localUrl, type Step } from './api'

  let { step = $bindable(), number, previewVideo, duration, nextT, ondelete }: {
    step: Step
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

  const kinds = { click: 'Click', typed: 'Typing', shortcut: 'Shortcut' }
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

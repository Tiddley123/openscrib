<script lang="ts">
  import ModelsSection from './ModelsSection.svelte'
  import { ACCENTS, DEFAULTS, qualityLabel, saveSettings, settings, type Theme } from './settings.svelte'

  let { onclose }: { onclose: () => void } = $props()

  const themes: { id: Theme; label: string }[] = [
    { id: 'system', label: 'System' },
    { id: 'light', label: 'Light' },
    { id: 'dark', label: 'Dark' },
  ]
  const widths = [
    { value: 1920, label: '1920 px wide (plays everywhere)' },
    { value: 2560, label: '2560 px wide' },
    { value: 0, label: 'Original size' },
  ]

  // Every change applies immediately so the effect is visible behind the dialog.
  $effect(() => {
    JSON.stringify(settings)
    saveSettings()
  })

  // Stop shortcut capture. Mirrors `Hotkey::parse` in openscrib-capture,
  // which accepts Win, Cmd, Meta and Super for the same modifier and labels
  // it the way the platform does.
  const META = /Mac/i.test(navigator.userAgent) ? 'Cmd' : 'Win'
  let listening = $state(false)
  let keyError = $state<string | null>(null)
  const SPECIAL: Record<string, string> = {
    Pause: 'Pause', ScrollLock: 'ScrollLock', PrintScreen: 'PrintScreen', Insert: 'Insert', Delete: 'Delete',
    Home: 'Home', End: 'End', PageUp: 'PageUp', PageDown: 'PageDown', Escape: 'Esc', Space: 'Space',
  }

  function keyName(code: string): string | null {
    if (/^F([1-9]|1[0-2])$/.test(code)) return code
    if (/^Key[A-Z]$/.test(code)) return code.slice(3)
    if (/^Digit[0-9]$/.test(code)) return code.slice(5)
    return SPECIAL[code] ?? null
  }

  // While the picker is listening, the whole window is watched rather than the
  // button: WebKit on macOS gives a button no keyboard focus when it is
  // clicked, so a handler on the button itself never heard the combination.
  // The capture phase matters too, or the dialog's own Escape-to-close would
  // run first and no shortcut using Esc could be set.
  $effect(() => {
    if (!listening) return
    window.addEventListener('keydown', captureKey, true)
    return () => window.removeEventListener('keydown', captureKey, true)
  })

  // Escape closes the dialog, for the same reason from the window rather than
  // the dialog: with nothing in it focused, nothing in it hears a key. While
  // the picker listens, Escape belongs to the combination instead.
  $effect(() => {
    if (listening) return
    const close = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onclose()
    }
    window.addEventListener('keydown', close)
    return () => window.removeEventListener('keydown', close)
  })

  function captureKey(e: KeyboardEvent) {
    if (!listening) return
    e.preventDefault()
    e.stopPropagation()
    if (['Control', 'Alt', 'Shift', 'Meta'].includes(e.key)) return
    const name = keyName(e.code)
    if (!name) {
      keyError = `${e.code} can't be used; try a letter, a number, an F-key or Pause.`
      return
    }
    const mods = [e.ctrlKey && 'Ctrl', e.altKey && 'Alt', e.shiftKey && 'Shift', e.metaKey && META].filter(Boolean)
    // Esc and Space are deliberately missing: `Hotkey::parse` wants a
    // modifier with them, or they would fire while someone is typing.
    const special = /^F\d+$/.test(name) || ['Pause', 'ScrollLock', 'PrintScreen', 'Insert', 'Delete', 'Home', 'End', 'PageUp', 'PageDown'].includes(name)
    if (!e.ctrlKey && !e.altKey && !e.metaKey && !special) {
      keyError = `Add Ctrl, Alt or ${META}, or that key would stop recording while you type.`
      return
    }
    settings.stopKey = [...mods, name].join('+')
    keyError = null
    listening = false
  }
</script>

<div
  class="scrim"
  role="presentation"
  onclick={(e) => {
    if (e.target === e.currentTarget) onclose()
  }}
>
  <div class="dialog" role="dialog" aria-modal="true" aria-labelledby="settings-title">
    <h2 id="settings-title">Settings</h2>

    <section>
      <div class="label">Theme</div>
      <div class="segmented" role="radiogroup" aria-label="Theme">
        {#each themes as t}
          <button role="radio" aria-checked={settings.theme === t.id} class:on={settings.theme === t.id} onclick={() => (settings.theme = t.id)}>
            {t.label}
          </button>
        {/each}
      </div>
    </section>

    <section>
      <div class="label">Accent colour</div>
      <div class="swatches">
        {#each ACCENTS as c}
          <button
            class="swatch"
            class:on={settings.accent.toLowerCase() === c}
            style:background={c}
            aria-label="Use {c}"
            onclick={() => (settings.accent = c)}
          ></button>
        {/each}
        <label class="custom" title="Pick any colour">
          <input type="color" bind:value={settings.accent} />
          Custom
        </label>
      </div>
      <p class="hint">Used for buttons and highlights in the app. Each guide also has its own highlight colour, set from the editor's toolbar.</p>
    </section>

    <section>
      <div class="row">
        <div class="label">Video quality</div>
        <span class="value">{settings.quality} · {qualityLabel(settings.quality)}</span>
      </div>
      <input class="slider" type="range" min="1" max="100" step="1" bind:value={settings.quality} />
      <div class="scale"><span>Smaller files</span><span>Sharper text</span></div>
      <p class="hint">Applies to new recordings and to video exports. Static screens stay small at any setting, because unchanged frames are skipped.</p>
    </section>

    <section>
      <div class="row">
        <div class="label">PDF image quality</div>
        <span class="value">{settings.pdfQuality === 100 ? 'Lossless' : settings.pdfQuality}</span>
      </div>
      <input class="slider" type="range" min="1" max="100" step="1" bind:value={settings.pdfQuality} />
      <div class="scale"><span>Smaller files</span><span>Sharper screenshots</span></div>
      <p class="hint">Applies to screenshots in exported PDFs. The Export window shows what the size comes to.</p>
    </section>

    <section>
      <div class="label">Stop recording shortcut</div>
      <div class="hotkey">
        <button class="keys" class:listening onclick={() => (listening = !listening)}>
          {#if listening}
            Press the new combination…
          {:else}
            {#each settings.stopKey.split('+') as k, i}{#if i}<span class="plus">+</span>{/if}<kbd>{k}</kbd>{/each}
          {/if}
        </button>
        {#if settings.stopKey !== DEFAULTS.stopKey}
          <button class="ghost small" onclick={() => (settings.stopKey = DEFAULTS.stopKey)}>Reset</button>
        {/if}
      </div>
      <p class="hint" class:warn={keyError}>{keyError ?? 'Click, then press the keys. Use Ctrl, Alt or Win with a key, or a key nobody types like Pause or F9.'}</p>
    </section>

    <section>
      <div class="label">Exported video size</div>
      <select bind:value={settings.exportWidth}>
        {#each widths as w}
          <option value={w.value}>{w.label}</option>
        {/each}
      </select>
    </section>

    <h3>Models</h3>
    <ModelsSection />

    <div class="actions">
      <button class="ghost" onclick={() => Object.assign(settings, DEFAULTS)}>Reset to defaults</button>
      <button class="primary" onclick={onclose}>Done</button>
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
    z-index: 45;
    padding: 24px;
  }
  .dialog {
    width: min(540px, 100%);
    max-height: calc(100vh - 48px);
    overflow: auto;
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: 16px;
    box-shadow: var(--shadow);
    padding: 24px;
    display: grid;
    gap: 22px;
  }
  h2 {
    margin: 0;
    font-size: 20px;
    letter-spacing: -0.02em;
  }
  h3 {
    margin: 4px 0 -8px;
    font-size: 15px;
    padding-top: 18px;
    border-top: 1px solid var(--line);
  }
  section {
    display: grid;
    gap: 8px;
  }
  .row {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
  }
  .value {
    font-size: 13px;
    font-weight: 600;
  }
  .segmented {
    display: inline-flex;
    width: fit-content;
    background: var(--panel-2);
    border: 1px solid var(--line);
    border-radius: 10px;
    padding: 3px;
    gap: 2px;
  }
  .segmented button {
    border: none;
    background: transparent;
    padding: 6px 16px;
    border-radius: 7px;
  }
  .segmented button.on {
    background: var(--ink);
    color: var(--bg);
  }
  .swatches {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
  }
  .swatch {
    width: 26px;
    height: 26px;
    padding: 0;
    border-radius: 50%;
    border: 2px solid var(--panel);
    box-shadow: 0 0 0 1px var(--line-strong);
  }
  .swatch.on {
    box-shadow: 0 0 0 2px var(--ink);
  }
  .custom {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 13px;
    color: var(--ink-2);
  }
  .custom input {
    width: 28px;
    height: 28px;
    padding: 0;
    border-radius: 6px;
    border: 1px solid var(--line-strong);
    background: none;
  }
  .slider {
    width: 100%;
    accent-color: var(--accent);
    padding: 0;
    border: none;
  }
  .scale {
    display: flex;
    justify-content: space-between;
    font-size: 12px;
    color: var(--muted);
    margin-top: -4px;
  }
  .hotkey {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .keys {
    min-width: 220px;
    justify-content: center;
    gap: 4px;
  }
  .keys.listening {
    border-color: var(--accent);
    box-shadow: var(--focus);
    color: var(--accent);
  }
  .plus {
    color: var(--muted);
  }
  .small {
    padding: 4px 10px;
    font-size: 12.5px;
  }
  .hint.warn {
    color: var(--danger);
  }
  .hint {
    margin: 0;
    font-size: 12.5px;
    color: var(--muted);
  }
  select {
    width: fit-content;
  }
  .actions {
    display: flex;
    justify-content: space-between;
  }
</style>

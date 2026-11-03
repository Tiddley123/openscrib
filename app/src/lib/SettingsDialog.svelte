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
</script>

<div
  class="scrim"
  role="presentation"
  onclick={(e) => {
    if (e.target === e.currentTarget) onclose()
  }}
  onkeydown={(e) => e.key === 'Escape' && onclose()}
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

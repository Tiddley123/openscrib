// App-wide preferences, kept per user in localStorage. Project-specific
// choices (title, highlight colour) live in the manifest instead.

export type Theme = 'system' | 'light' | 'dark'

export interface Settings {
  theme: Theme
  /** UI accent colour, `#rrggbb`. */
  accent: string
  /** Video quality 1-100 for recording and export. */
  quality: number
  /** Exported video width cap; 0 keeps the recording's width. */
  exportWidth: number
  /** Whisper model id used by Process. */
  whisperModel: string
  /** Key combination that stops a recording. */
  stopKey: string
}

export const DEFAULTS: Settings = { theme: 'system', accent: '#ff5c35', quality: 75, exportWidth: 1920, whisperModel: 'small.en', stopKey: 'Ctrl+Shift+F12' }

export const ACCENTS = ['#ff5c35', '#e5484d', '#f5a524', '#30a46c', '#12a594', '#0a84ff', '#6e56cf', '#d6409f', '#1c1c1e']

const KEY = 'openscrib.settings'

function load(): Settings {
  try {
    const raw = localStorage.getItem(KEY)
    return raw ? { ...DEFAULTS, ...JSON.parse(raw) } : { ...DEFAULTS }
  } catch {
    return { ...DEFAULTS }
  }
}

export const settings = $state<Settings>(load())

export function saveSettings() {
  try {
    localStorage.setItem(KEY, JSON.stringify(settings))
  } catch {
    // Private mode or storage disabled: settings just won't persist.
  }
  apply()
}

/** Push theme and accent onto the document root. */
export function apply() {
  const root = document.documentElement
  if (settings.theme === 'system') delete root.dataset.theme
  else root.dataset.theme = settings.theme
  root.style.setProperty('--accent', settings.accent)
  root.style.setProperty('--accent-hover', shade(settings.accent, -0.08))
}

/** Lighten (positive) or darken (negative) a hex colour. */
export function shade(hex: string, amount: number): string {
  const n = parseInt(hex.slice(1), 16)
  const c = [(n >> 16) & 255, (n >> 8) & 255, n & 255].map((v) =>
    Math.round(Math.min(255, Math.max(0, amount < 0 ? v * (1 + amount) : v + (255 - v) * amount))),
  )
  return `#${c.map((v) => v.toString(16).padStart(2, '0')).join('')}`
}

export function qualityLabel(q: number): string {
  if (q >= 90) return 'Crisp'
  if (q >= 70) return 'High'
  if (q >= 45) return 'Balanced'
  return 'Small files'
}

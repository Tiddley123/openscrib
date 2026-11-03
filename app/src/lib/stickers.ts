// Pictograph stickers: shared by the canvas (drawing) and the picker. The
// exported versions are drawn in Rust (openscrib-export/src/icons.rs) to
// match.

import type { StickerIcon } from './api'

export const STICKERS: { id: StickerIcon; label: string; color: string; glyph: string }[] = [
  { id: 'warning', label: 'Warning', color: '#f5a524', glyph: '!' },
  { id: 'important', label: 'Important', color: '#e5484d', glyph: '!' },
  { id: 'info', label: 'Info', color: '#0a84ff', glyph: 'i' },
  { id: 'tip', label: 'Tip', color: '#30a46c', glyph: '★' },
  { id: 'check', label: 'Do this', color: '#30a46c', glyph: '✓' },
  { id: 'cross', label: "Don't", color: '#e5484d', glyph: '✕' },
  { id: 'question', label: 'Question', color: '#6e56cf', glyph: '?' },
  { id: 'quote', label: 'Quote', color: '', glyph: '“' },
]

export function stickerColor(id: StickerIcon, accent: string): string {
  return STICKERS.find((s) => s.id === id)?.color || accent
}

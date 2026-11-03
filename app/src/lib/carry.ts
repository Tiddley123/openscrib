// Redactions and crops that carry forward: drawn on one step, they apply to
// every later step showing the same screen, until removed on some step
// (which removes them from that step onward) or replaced by another crop.

import type { Manifest, Rect, Step } from './api'

export type FrameSize = (step: Step) => [number, number] | null

function newGroup(): number {
  return Math.floor(Date.now() % 1e9) + Math.floor(Math.random() * 1000)
}

/** Steps after `index` that show the same screen as `index`. */
function laterSameScreen(m: Manifest, index: number, size: FrameSize): Step[] {
  const here = m.steps[index]
  const s = size(here)
  if (!here.frame || !s) return []
  return m.steps.slice(index + 1).filter((t) => {
    const ts = size(t)
    return !!t.frame && t.monitor === here.monitor && !!ts && ts[0] === s[0] && ts[1] === s[1]
  })
}

export function addRedaction(m: Manifest, index: number, rect: Rect, carry: boolean, size: FrameSize) {
  if (!carry) {
    m.steps[index].annotations.push({ type: 'blur', rect })
    return
  }
  const group = newGroup()
  for (const s of [m.steps[index], ...laterSameScreen(m, index, size)]) {
    s.annotations.push({ type: 'blur', rect: { ...rect }, group })
  }
}

export function setCrop(m: Manifest, index: number, rect: Rect, carry: boolean, size: FrameSize) {
  const here = m.steps[index]
  const previous = here.crop_group
  const group = carry ? newGroup() : undefined
  here.crop = rect
  here.crop_group = group
  if (!carry) return
  for (const s of laterSameScreen(m, index, size)) {
    // Stop at a later step that has its own, different crop.
    if (s.crop && (previous === undefined || s.crop_group !== previous)) break
    s.crop = { ...rect }
    s.crop_group = group
  }
}

export function clearCrop(m: Manifest, index: number) {
  const here = m.steps[index]
  const group = here.crop_group
  here.crop = undefined
  here.crop_group = undefined
  if (group === undefined) return
  for (const s of m.steps.slice(index + 1)) {
    if (s.crop_group === group) {
      s.crop = undefined
      s.crop_group = undefined
    }
  }
}

/** Remove a note; a carried redaction also goes from every later step. */
export function removeNote(m: Manifest, index: number, note: number) {
  const here = m.steps[index]
  const a = here.annotations[note]
  here.annotations.splice(note, 1)
  if (a?.type !== 'blur' || a.group === undefined) return
  for (const s of m.steps.slice(index + 1)) {
    s.annotations = s.annotations.filter((b) => !(b.type === 'blur' && b.group === a.group))
  }
}

/** How many later steps carry this step's crop or a given redaction. */
export function carriedCount(m: Manifest, index: number, group: number | undefined, kind: 'crop' | 'blur'): number {
  if (group === undefined) return 0
  return m.steps
    .slice(index + 1)
    .filter((s) => (kind === 'crop' ? s.crop_group === group : s.annotations.some((a) => a.type === 'blur' && a.group === group)))
    .length
}

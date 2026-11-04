// Types mirroring openscrib-core's manifest, and typed wrappers around the
// Tauri commands in src-tauri/src/lib.rs.

import { convertFileSrc, invoke } from '@tauri-apps/api/core'
import { inTauri, mockApi } from './mock'

export type Point = { x: number; y: number }
export type Rect = { x: number; y: number; w: number; h: number }

export type Annotation =
  | { type: 'marker'; at: Point }
  | { type: 'arrow'; from: Point; to: Point }
  | { type: 'box'; rect: Rect }
  | { type: 'blur'; rect: Rect; group?: number }
  | { type: 'sticker'; at: Point; icon: StickerIcon; size: number }

export type StickerIcon = 'warning' | 'important' | 'info' | 'tip' | 'check' | 'cross' | 'question' | 'quote'
export type Callout = 'note' | 'tip' | 'warning' | 'important' | 'quote'

export interface NarrationSegment {
  start_ms: number
  end_ms: number
  text: string
  step?: number
}

export interface Click {
  x: number
  y: number
  button: 'left' | 'right' | 'middle' | 'other'
  double?: boolean
  in_window?: Point
  in_frame: Point
}

export interface WindowInfo {
  rect: Rect
  client?: Rect
  title: string
  process: string
}

export interface Clip {
  start_ms: number
  end_ms: number
}

export interface Step {
  id: number
  t_ms: number
  kind: 'click' | 'typed' | 'shortcut' | 'note'
  click?: Click
  keys?: string
  monitor?: number
  window?: WindowInfo
  frame?: string
  pre_frame?: string
  transcript?: string
  tooltip?: string
  ocr?: string
  vision?: string
  text: string
  text_edited: boolean
  clip?: Clip
  crop?: Rect
  crop_group?: number
  annotations: Annotation[]
  callout?: Callout
  hide_marker?: boolean
  /** Left out of guides and videos, kept so redactions can still be set on it. */
  deleted?: boolean
}

export interface MonitorInfo {
  id: number
  name: string
  rect: Rect
  scale: number
  primary: boolean
}

export interface Manifest {
  version: number
  app_version: string
  title?: string
  accent?: string
  created: string
  duration_ms: number
  monitors: MonitorInfo[]
  video?: { file: string; monitor: number; width: number; height: number; max_fps: number; frames: number; first_ms: number }
  /** The part of the screen exported videos show. */
  video_crop?: Rect
  audio?: { file: string; device: string; device_rate: number; channels: number; first_ms: number }
  steps: Step[]
  warnings?: string[]
  narration?: NarrationSegment[]
}

export interface ProjectView {
  source: string
  work_dir: string
  manifest: Manifest
  default_crops: (Rect | null)[]
  frame_sizes: ([number, number] | null)[]
  preview_video: string | null
}

export interface RecentItem {
  path: string
  name: string
  title: string | null
  steps: number
  duration_ms: number
  modified: number
}

export interface Devices {
  monitors: MonitorInfo[]
  mics: [string, boolean][]
  video_available: boolean
}

/// What the OS has granted. `required` is false off macOS.
export interface Permissions {
  required: boolean
  screen: boolean
  input: boolean
  note: string
}

export interface RecordRequest {
  video: boolean
  audio: boolean
  monitor: number | null
  mic: string | null
  quality: number
  stop_key: string
}

export interface ModelStatus {
  id: string
  kind: 'whisper' | 'ocr_detection' | 'ocr_recognition' | 'vision_model' | 'vision_projector' | 'tts_engine' | 'tts_voice'
  label: string
  note: string
  file: string
  url: string
  size_mb: number
  installed: boolean
}

/** A voice the system speech synthesiser offers; nothing to download. */
export interface SystemVoice {
  name: string
  locale: string
}

export interface ProcessReport {
  segments: number
  steps_with_narration: number
  labels: number
  tooltips: number
  redrafted: number
  skipped: string[]
}

export interface ProcessProgress {
  stage: string
  fraction: number
  message: string
}

export type ExportFormat = 'md' | 'html' | 'pdf' | 'mkv' | 'mp4'

const dev = import.meta.env.DEV && !inTauri

export const api = {
  listRecordings: () => (dev ? mockApi.listRecordings() : invoke<RecentItem[]>('list_recordings')),
  openProject: (path: string) => (dev ? mockApi.openProject() : invoke<ProjectView>('open_project', { path })),
  saveProject: (manifest: Manifest) => invoke<void>('save_project', { manifest }),
  exportProject: (req: {
    manifest: Manifest
    formats: ExportFormat[]
    out_dir: string
    full_frame: boolean
    quality: number
    max_width: number
    pdf_quality: number
    voiceover: string | null
  }) =>
    invoke<void>('export_project', { req }),
  estimateExport: (req: { manifest: Manifest; full_frame: boolean; quality: number; max_width: number; pdf_quality: number }) =>
    dev ? Promise.resolve<Record<string, number>>({}) : invoke<Record<string, number>>('estimate_export', { req }),
  startRecording: (req: RecordRequest) =>
    invoke<string>('start_recording', { req }),
  stopRecording: () => invoke<void>('stop_recording'),
  listDevices: () => (dev ? mockApi.listDevices() : invoke<Devices>('list_devices')),
  defaultExportDir: (title: string | null) =>
    dev ? mockApi.defaultExportDir() : invoke<string>('default_export_dir', { title }),
  listModels: () => (dev ? mockApi.listModels() : invoke<ModelStatus[]>('list_models')),
  listVoices: () => (dev ? mockApi.listVoices() : invoke<SystemVoice[]>('list_voices')),
  downloadModel: (id: string) => invoke<void>('download_model', { id }),
  removeModel: (id: string) => invoke<void>('remove_model', { id }),
  processProject: (manifest: Manifest, whisper: string | null) => invoke<void>('process_project', { manifest, whisper }),
  draftText: (step: Step) => invoke<string>('draft_text', { step }),
  downloadOpenH264: () => invoke<string>('download_openh264'),
  permissions: () =>
    dev ? mockApi.permissions() : invoke<Permissions>('permissions'),
  requestPermission: (which: 'screen' | 'input') =>
    invoke<Permissions>('request_permission', { which }),
  openPermissionSettings: (which: 'screen' | 'input') =>
    invoke<void>('open_permission_settings', { which }),
  importImage: (path: string, stepId: number) =>
    invoke<{ rel: string; width: number; height: number }>('import_image', { path, stepId }),
}

/** URL the WebView can load for a file inside the open project. */
export function fileUrl(workDir: string, rel: string): string {
  return localUrl(`${workDir}/${rel}`)
}

/** URL the WebView can load for an absolute local path. */
export function localUrl(path: string): string {
  return inTauri ? convertFileSrc(path) : path
}

/** Step numbers as exported: deleted steps don't count. */
export function stepNumbers(steps: Step[]): Map<number, number> {
  const numbers = new Map<number, number>()
  for (const s of steps) {
    if (!s.deleted) numbers.set(s.id, numbers.size + 1)
  }
  return numbers
}

export function formatBytes(n: number): string {
  if (n < 1e6) return `${Math.max(1, Math.round(n / 1e3))} KB`
  return n < 1e9 ? `${(n / 1e6).toFixed(n < 1e7 ? 1 : 0)} MB` : `${(n / 1e9).toFixed(1)} GB`
}

export function formatTime(ms: number): string {
  const s = Math.max(0, ms) / 1000
  const m = Math.floor(s / 60)
  return `${m}:${(s - m * 60).toFixed(1).padStart(4, '0')}`
}

/** Rebuild each step's transcript from the narration assigned to it. */
export function syncTranscripts(m: Manifest) {
  for (const s of m.steps) {
    const parts = (m.narration ?? []).filter((n) => n.step === s.id).map((n) => n.text.trim())
    s.transcript = parts.length ? parts.join(' ') : undefined
  }
}

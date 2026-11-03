// Dev-only stand-in for the Tauri backend so the UI can be worked on in a
// plain browser (`npm run dev`). Serves a sample project from
// app/dev-mock/ (gitignored; put an unpacked .scrib in dev-mock/project and
// an MP4 at dev-mock/preview.mp4).

import type { Manifest, ModelStatus, ProjectView, RecentItem, SystemVoice } from './api'

export const inTauri = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window

const MOCK_DIR = '/mock/project'

export const mockApi = {
  async listRecordings(): Promise<RecentItem[]> {
    const m = await manifest()
    return [{ path: 'mock', name: 'sample-recording', title: m.title ?? null, steps: m.steps.length, duration_ms: m.duration_ms, modified: Date.now() / 1000 - 300 }]
  },
  async openProject(): Promise<ProjectView> {
    const m = await manifest()
    return {
      source: 'mock.scrib',
      work_dir: MOCK_DIR,
      manifest: m,
      default_crops: m.steps.map(() => null),
      frame_sizes: m.steps.map((s) => {
        const mon = m.monitors.find((x) => x.id === s.monitor)
        return mon ? [mon.rect.w, mon.rect.h] : null
      }),
      preview_video: '/mock/preview.mp4',
    }
  },
  async listDevices() {
    const m = await manifest()
    return { monitors: m.monitors, mics: [['Default microphone', true]] as [string, boolean][], video_available: true }
  },
  async permissions() {
    return { required: false, screen: true, input: true, note: '' }
  },
  defaultExportDir: async () => 'C:\\exports\\sample-export',
  async listModels(): Promise<ModelStatus[]> {
    const m = (id: string, kind: ModelStatus['kind'], label: string, size_mb: number, installed: boolean) =>
      ({ id, kind, label, note: '', file: '', url: '', size_mb, installed })
    return [
      m('base.en', 'whisper', 'Whisper base (English)', 142, false),
      m('small.en', 'whisper', 'Whisper small (English)', 466, true),
      m('large-v3-turbo', 'whisper', 'Whisper large v3 turbo (multilingual)', 547, false),
      m('ocr-detection', 'ocr_detection', 'OCR text detection', 3, true),
      m('ocr-recognition', 'ocr_recognition', 'OCR text recognition', 10, true),
    ]
  },

  async listVoices(): Promise<SystemVoice[]> {
    return [
      { name: 'Samantha', locale: 'en_US' },
      { name: 'Daniel', locale: 'en_GB' },
      { name: 'Anna', locale: 'de_DE' },
    ]
  },
}

async function manifest(): Promise<Manifest> {
  const r = await fetch(`${MOCK_DIR}/project.json`)
  return r.json()
}

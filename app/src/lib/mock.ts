// Dev-only stand-in for the Tauri backend so the UI can be worked on in a
// plain browser (`npm run dev`). Serves a sample project from
// app/dev-mock/ (gitignored; put an unpacked .scrib in dev-mock/project and
// an MP4 at dev-mock/preview.mp4).

import type { Manifest, ProjectView, RecentItem } from './api'

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
  defaultExportDir: async () => 'C:\\exports\\sample-export',
}

async function manifest(): Promise<Manifest> {
  const r = await fetch(`${MOCK_DIR}/project.json`)
  return r.json()
}

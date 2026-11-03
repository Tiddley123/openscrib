# Rust Step Recorder — Project Outline

Nov 2, 2026 · @Tyler Darnell

## Overview and goals

We are building a portable, open-source, Rust-based step recorder: a local-first alternative to Scribe that produces both a written guide (PDF/Markdown/HTML) and a video from one recording, and works through remote sessions such as TeamViewer.

No existing open-source tool covers this. Mimik (MIT) does browser-only guides without video; OpenSteps is a Windows-only early beta with Markdown/HTML export; GuideGen makes a narrated slideshow, not a real screen recording.\
\
Visual UI should be similar to that of solidcore with a clean aesthetic.&#32;

Goals:

1. **Record once, export twice:** one capture produces a step-by-step guide with annotated screenshots and an edited video.
2. **Works through remote desktops:** everything is captured on the viewer side from pixels and coordinates, so TeamViewer and similar sessions work.
3. **Portable, no admin:** a single app folder with model files beside the executable. No servers, installers or admin rights.
4. **Light by default, smart when enabled:** the core runs with zero models. OCR, dictation, vision and voiceover are opt-in tiers, with peak memory around 4 GB.
5. **Narration-driven steps:** the user's spoken commentary, OCR and a small vision model combine to write step text automatically.

## Scope

**Users:** power users such as IT support, trainers and consultants, who are comfortable dropping model files into a folder and editing a config file.

**Platforms:**

- Windows 10/11 first (main TeamViewer use case); reference machine: RTX 3080 desktop.
- macOS on Apple Silicon second; reference machine: M4 with 16 GB unified memory.
- Linux X11 later. Wayland capture and input hooks are restricted, so it is best-effort.

**Non-goals for v1:**

- Reading UI element names from the DOM or accessibility tree. It doesn't work through remote sessions, and the names are rarely human readable anyway.
- Cloud accounts, hosting or sharing links.
- Interactive click-through tours (Guide Me style replay).
- Live/real-time transcription or AI while recording.

## Pipeline

Capture stays light during the live session; all AI work happens after the user hits Process, one model at a time.

&#91;embedded content: processing pipeline · 5 stages\]

Whisper is loaded and unloaded first; the multimodal model then stays resident from step identification through editing, so it is only loaded once.

## Capability tiers

The core works with no models at all; each tier switches on when its model file is present and enabled in settings. A missing tier is skipped, never an error.

| Tier | Adds | Model size (approx.) | Cargo feature |
| --- | --- | --- | --- |
| Core | Screen video, click PNGs, click markers, manual step text, PDF/Markdown/HTML/MP4 export | None | (default) |
| OCR | Labels read around clicks; tooltip text from pre-click frames | 10–50 MB | `ocr` |
| Dictation | Narration transcribed with Whisper | 75 MB (tiny) to 1.6 GB (large-v3-turbo) | `whisper` |
| Vision + LLM | Icon identification and written step text, one multimodal model | 1–4 GB (2B–4B at 4-bit) | `llm` |
| Voiceover | Text-to-speech narration for exported video (Piper) | 20–60 MB per voice | `tts` |

Power-user controls:

- `models/` folder scanned at startup; any compatible `.gguf` or Whisper model can be swapped in.
- `config.toml` for backend choice (CPU / Vulkan / Metal / optional cloud API key) and editable prompts for tone, language and house style.
- Each stage can be re-run on its own later with a better model, without re-recording.

## Model stack and memory budget

Target peak memory is about 4 GB. We get there by running models one after another, never at the same time, and by using one multimodal model for both vision and writing.

Load order:

1. **Whisper:** transcribe narration, then unload. Running it first lets the vision prompts include what the user said near each click.
2. **Multimodal model (2B–4B, 4-bit, \~2–3.5 GB):** identify icons and write step text from crop + OCR + transcript. It **stays loaded** through the editing stage for rewording and formatting.
3. **Piper TTS (optional):** tiny, CPU-only, used only when exporting a voiced video.

Keeping it cheap:

- Only send ambiguous steps to the vision model. Skip it when tooltip OCR or narration already names the target (expect most steps to skip it).
- Batch all crops through one model load.
- Rule of thumb: about 0.6 GB per billion parameters at 4-bit. Mixture-of-experts models run fast but still need their full weights in memory, so they are out of budget.

Candidate models (verify current versions before committing): Whisper small or large-v3-turbo; Moondream, SmolVLM, Qwen-VL 2B–4B or Gemma 3 4B for vision; Florence-2 as an ultra-light fallback.

## Capture and video encoding

Video is the source of truth; screenshots are sharp extras pulled at click time.

- **Continuous screen video:** H.264 via OpenH264, 10–15 fps, variable frame rate. Unchanged frames are skipped using a quick hash or sampled-pixel diff, so idle time costs almost nothing.
- **Lossless PNG at every click:** full screen, kept for OCR, vision and crisp PDF images, since lossy video blurs small text.
- **Pre-click frame:** grab the frame when the mouse has paused about 500 ms before a click, to catch tooltips on icon buttons.
- **Audio:** narration recorded to Opus.
- **Input:** global mouse and keyboard hooks. A burst of keystrokes collapses into one "typed X into Y" step.
- **One clock:** clicks, keys, frames and audio all timestamped in milliseconds from a single monotonic start time.

OpenH264 notes:

- Cisco only covers H.264 patent royalties for **their prebuilt binaries**. Distributed builds must load Cisco's DLL/dylib at runtime (fetched on first run or dropped in by the user), not compile it from source.
- Baseline profile only, so files are slightly larger than x264. That's fine for mostly static screen content.
- Input must be YUV420 with even width and height. The output is a raw H.264 stream that needs muxing into MP4/MKV with our own timestamps.
- Later options: hardware encoding (NVENC, VideoToolbox) via a bundled LGPL ffmpeg; AV1 (`rav1e`) for smaller exports.

## Project file and data format

A project is a zip with its own extension (like .docx). Inside, a JSON manifest ties every asset together by timestamp.

```
myguide.guide (zip)
├── project.json      # manifest: steps, settings, version
├── screen.mkv        # continuous screen recording
├── audio.opus        # narration
└── frames/           # lossless PNG per click (+ pre-click frames)
```

Per-step record:

```json
{
  "t_ms": 12450,
  "click": { "x": 812, "y": 344, "button": "left" },
  "window": { "x": 0, "y": 40, "w": 1920, "h": 1040 },
  "frame": "frames/0003.png",
  "pre_frame": "frames/0003_pre.png",
  "transcript": "now hit the cog for settings",
  "ocr": "Settings",
  "vision": "gear icon, top right toolbar",
  "text": "Click **Settings** (gear icon).",
  "text_edited": true,
  "clip": { "start_ms": 11000, "end_ms": 14000 },
  "annotations": []
}
```

Rules:

- Raw stage outputs (`transcript`, `ocr`, `vision`) are kept separate from the final `text`. Re-running a stage never overwrites the user's edits (`text_edited`).
- All times are in milliseconds from the recording start.
- `version` at the top of the manifest allows schema migrations.

## Remote sessions (TeamViewer)

The recorder runs on the viewer's machine and treats the remote desktop as pixels in a local window. Screenshots, click markers, keystrokes and video all work; element names do not, which is why OCR, vision and narration carry step text.

Requirements:

- Store click coordinates relative to the captured window as well as the screen, so markers stay correct under TeamViewer's scaled view.
- Optional crop to the client area, excluding the TeamViewer toolbar and borders.
- On Windows, run at the same privilege level as TeamViewer. If it's elevated and we're not, input hooks may miss events while it has focus.
- Detect black or blank frames (capture blocked by policy) and warn early.
- Test matrix: TeamViewer full-screen, windowed, scaled; plus RDP and AnyDesk as stretch targets.

## Portability and permissions

The app ships as one folder that runs from anywhere, including a USB stick, with no installer, services or admin rights.

- Models run **in-process**: llama.cpp and whisper.cpp linked in, with models loaded from `models/`. No Ollama or local server.
- GPU via **Vulkan** on Windows/Linux (needs only a normal GPU driver, not the CUDA toolkit) and **Metal** on macOS; CPU fallback always available.
- Settings, logs and projects default to paths inside the app folder, with an option to use the user profile instead.
- **Windows:** input hooks and screen capture need no admin.
- **macOS:** the user must grant Accessibility and Screen Recording on first run. That's a one-time user prompt and can't be bundled away; the first-run screen should explain it.
- Cisco's OpenH264 binary is fetched on first run, or placed manually for offline machines.

## Suggested crates

Starting points only; check maintenance status and licences before locking in.

| Area | Crate | Notes |
| --- | --- | --- |
| Input hooks | `rdev` | Global mouse/keyboard, cross-platform |
| Screen capture | `xcap` | Screen and window capture; `scap` as an alternative for frame streams |
| Video encode | `openh264` | Load Cisco's binary at runtime for distribution |
| Annotation | `tiny-skia`, `imageproc` | Click markers, arrows, boxes, blur/redaction |
| OCR | `ocrs` | Pure Rust; `leptess` (Tesseract) as a fallback |
| Speech-to-text | `whisper-rs` | whisper.cpp bindings |
| Vision/LLM | `llama-cpp-2` | llama.cpp bindings, multimodal, Vulkan/Metal; `candle` as a pure-Rust alternative |
| TTS | Piper | Via bindings or a bundled binary |
| PDF export | `typst` (as library) | Layout quality cheaply; `printpdf` for low-level control |
| UI | `tauri` or `egui` | Tauri for a web-style step editor, egui for pure Rust |
| Project file | `zip`, `serde_json` | Manifest + assets |

## Milestones

Each milestone ends with something usable; no dates set yet.

1. **M1 – Capture core (Windows):** input hooks, click PNGs, single-clock timestamps, project zip + manifest.
   - Done when: a recording through TeamViewer produces correct click markers on every PNG.
2. **M2 – Video:** OpenH264 encoding with frame skipping, MKV/MP4 muxing, Opus audio.
   - Done when: a 10-minute mostly static session stays under \~30 MB with in-sync audio.
3. **M3 – Editor + exports:** step list editor (reorder, edit text, trim clips, annotate, redact), PDF/Markdown/HTML and stitched MP4 export.
   - Done when: a fully manual guide can be exported in all formats.
4. **M4 – OCR + dictation:** `ocrs` around clicks and tooltips, Whisper transcription aligned to steps.
   - Done when: steps get draft text from transcript + OCR with no LLM.
5. **M5 – Vision/LLM:** embedded multimodal model, ambiguous-step routing, step writing, editable prompts.
   - Done when: peak memory stays at or under \~4 GB on the 3080 and the M4 16 GB.
6. **M6 – macOS + polish:** Metal backend, permission onboarding, Piper voiceover, portable release packaging.

## Open questions

- [ ] Project name and licence (MIT, Apache-2.0 or GPL)?
- [ ] UI toolkit: Tauri or egui?
- [ ] Container: MKV while recording (crash-safe), MP4 on export?
- [ ] Which 2B–4B multimodal model gives the best icon recognition per GB? Needs a small benchmark set of real UI crops.
- [ ] Ship llama.cpp with Vulkan only, or also a CUDA build for NVIDIA users?
- [ ] Redaction: manual only in v1, or auto-detect emails and IDs via OCR?
- [ ] Should a cloud API key option exist at all, given the local-first goal?
- [ ] Is there a way to embed a "GIF" type short video in a PDF document to illustrate actions more effectively?

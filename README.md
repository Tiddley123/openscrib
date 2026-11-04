# OpenSCRIB

A portable, local-first step recorder. Record once and export twice: you get a
step-by-step guide with annotated screenshots and an edited video. Capture works
from pixels and coordinates, so it also works through TeamViewer and other
remote sessions.

The design is in [docs/outline.md](docs/outline.md).

## The editor

```bash
cd app && npm install && npx tauri build --no-bundle
```

This builds `target/release/openscrib-app.exe`. From the editor you can record, open and save projects, and:

- **Steps:** drag to reorder, edit the text (with `**bold**` and `` `code` ``), or delete. A deleted step stays in the list, greyed out, so you can still set redactions on that moment of the video; restore it, or remove it for good, from the inspector.
- **Annotations:** draw arrows, boxes and markers, redact private areas (painted solid on export), or set a custom crop. To delete one, use the ✕ in the Notes list, right-click it, or select it and press Delete.
- **Clips:** trim each step's video against a preview, or click **Auto**.
- **Video crop:** drag the part of the screen the exported video should show. It applies to the whole video and leaves the guide's screenshots alone.
- **✦ Process:** transcribe your narration and read the label under each click, then redraft any step text you haven't edited. What you said takes priority over an icon guessed by the vision model.
- **Export:** PDF, HTML, Markdown, MP4 and MKV, with the size of each shown before you start.
- **Settings:** theme (system, light or dark), accent colour, video quality, PDF image quality, exported video size, and models to download. Each guide also has its own highlight colour.

For UI work, run `npm run tauri dev` for hot reload. Running `npm run dev` on its own works in a plain browser too, against a mock backend fed from `app/dev-mock/`.

## Status: M5 (vision model and writing)

- [x] `openscrib-vision`: a helper exe running Qwen2.5-VL 3B through llama.cpp (CPU), started by Process and closed afterwards. It's separate because whisper.cpp and llama.cpp each bundle `ggml`
- [x] Names icon-only clicks ("the **Start** button") and fixes labels where OCR read the wrong line ("the **Local Disk (C:)** drive", not "15.9 GB free")
- [x] Step sentences stay factual (action + name); the model only works your narration into them ("Double-click **doc**, where all the manuals are located.") and titles the guide
- [x] Every narration segment is kept, attached to the step it follows, re-attachable in the editor, and exported as `.srt`/`.vtt` captions next to the video
- [x] Editor additions: text and image steps, callout boxes (Note, Tip, Warning, Important, Quote), pictograph stickers, a draggable or hideable click marker, crop edges that snap to the window, and export folders named after the guide
- [ ] GPU acceleration (Vulkan) for the vision model

## Status: M6 (polish and release)

- [x] Portable Windows release: `scripts\package-windows.ps1` builds `dist\OpenSCRIB-<version>-windows-x64` (and a zip) with the app, CLI (`openscrib-cli.exe`), vision helper and app-local Visual C++ runtime
- [x] First-run OpenH264: **Get OpenH264** on the home screen (or `openscrib setup`) downloads Cisco's build and checks its SHA-256
- [x] Voiceover: the step text is read aloud in video exports and the captions follow the spoken text (`--voiceover` in the CLI, Settings -> Models in the editor). Windows and macOS speak through the voices they already ship, Australian ones included, with nothing to download (`--voiceover "Microsoft Catherine"`). Windows and Linux can also download Piper and US, UK or Scottish voices; Piper has no Australian voice
- [x] Redactions and crops carry forward to later steps of the same screen; redactions are also painted into the exported video while their step is active
- [x] macOS release: `scripts/package-macos.sh` builds, Developer ID signs and packages `OpenSCRIB.app` (with the CLI and vision helper inside it) as a .dmg and a .zip; `--notarize <profile>` submits it to Apple and staples the ticket
- [x] macOS capture: window and Dock lookups through CoreGraphics, points converted to pixels at the boundary, Metal for Whisper and the vision model, Screen Recording and Accessibility onboarding on the home screen, and capture checks in `openscrib devices`
- [x] macOS: a notarized public build (needs App Store Connect credentials)

## Status: M4 (narration and on-screen text)

- [x] Model manager: Whisper (tiny, base, small, medium, large-v3-turbo) and the OCR models, downloaded on demand into the portable `models/` folder (`openscrib models`, or Settings in the editor)
- [x] Narration decoded from the recording, transcribed with whisper.cpp, and matched to the nearest step. Silence and sound tags like `[typing]` are dropped
- [x] OCR (pure-Rust `ocrs`) reads the label under each click, and any tooltip that appeared before it
- [x] Step text drafted from these, with no language model: "Click **This PC**." instead of "Click in **…Remote Desktop…**". Text you've edited is never overwritten
- [x] Icons with no text are handled by the M5 vision model

### Building on macOS

Needs Xcode (or the Command Line Tools) for `clang` and the frameworks, and
CMake for whisper.cpp and llama.cpp. Both build their Metal backend with the
shaders embedded, so the bundle stays self-contained.

```bash
scripts/package-macos.sh            # dist/OpenSCRIB-<version>-macos-arm64.{dmg,zip}
scripts/package-macos.sh --no-sign  # a quick unsigned build
```

macOS gates what a recorder needs, and fails quietly when it is missing:
without **Screen Recording** every frame is blank and window titles are
empty, and without **Accessibility** the input hook never sees a click. The
home screen asks for both, `openscrib devices` reports them next to the
monitors and the window under the cursor, and a recording refuses to start
without them. macOS reopens the app when either is granted.

Coordinates differ too. macOS reports windows, clicks and the cursor in
points but captures in pixels, so `openscrib-capture`'s `mac` module converts
to pixels at the boundary and everything above it works as it does on
Windows. The app writes nothing inside its own bundle: recordings go to
`~/Documents/OpenSCRIB`, models and OpenH264 to
`~/Library/Application Support/OpenSCRIB`.

Voiceover uses the speech macOS already has, through `/usr/bin/say`, rather
than Piper: Piper's Mac release ships x86_64 binaries labelled aarch64,
unsigned and without the libraries they link, so nothing in that 19 MB
download can run. The system voices need no download, no signing and no
entitlement, so neither the Piper engine nor its voices appear in the model
catalogue on macOS. Settings -> Models lists the installed voices by
language, `openscrib models` prints them, and either place's name goes to
`--voiceover` (`openscrib export rec.scrib -f mp4 --voiceover Samantha`); an
unrecognised name falls back to the voice chosen in System Settings ->
Accessibility -> Spoken Content.

### Building with Whisper on Windows

whisper.cpp needs CMake, and its Rust bindings need `libclang` (from LLVM). If bindgen can't find headers, set paths in a local `.cargo/config.toml`:

```toml
[env]
LIBCLANG_PATH = "C:/Program Files/LLVM/bin"
BINDGEN_EXTRA_CLANG_ARGS = '"-IC:/Program Files/LLVM/lib/clang/23/include" "-I<MSVC>/include" "-I<Windows Kits>/Include/<ver>/ucrt"'
```

## Status: M3 (editor and exports)

- [x] Step images: window-aware crop (with limits on very wide or tall shapes), per-step crop override, annotations, and solid redaction
- [x] Markdown (folder of images), HTML (one self-contained file, light and dark), PDF (Typst, compiled in-process, works offline)
- [x] Video export: step clips stitched in step order (or the whole recording), drawn cursor and click ripples, scaled to 1920 px wide, narration copied as-is. MKV, or MP4 with the index at the front (checked with Mozilla's `mp4parse`)
- [x] Tauri 2 + Svelte 5 editor covering everything above, plus recording from the app (clicks on the editor itself are ignored)
- [x] Delete existing annotations.


## Status: M1 (capture core, Windows)

- [x] Global mouse and keyboard hooks (`rdev`), per-monitor DPI aware
- [x] Lossless full-monitor PNG at every click, plus a pre-click frame when the mouse rested ≥500 ms (for tooltips)
- [x] One monotonic clock; every event and step in ms from the recording start
- [x] Click coordinates stored for the screen, the target window and the frame
- [x] Window title, process, frame bounds and client area of the click target
- [x] Double-click merge; typing bursts collapse into one step (`hi{Tab}there{Enter}`); shortcuts become their own steps (`Ctrl+S`)
- [x] Blank or black frame warning (capture blocked by policy)
- [x] Crash-safe working directory, packed into a `.scrib` zip on stop
- [x] **Done-when check:** a recording through TeamViewer gives correct markers on every PNG (needs a manual run)
- [ ] Warn when the target app is elevated and we are not

## Status: M2 (video and narration)

- [x] Continuous screen video of one monitor at ≤12 fps (`--fps`, `--monitor`). Pixel-identical frames are skipped, so the video has a variable frame rate.
- [x] H.264 via Cisco's OpenH264 binary, loaded at runtime and verified by SHA-256
- [x] Microphone narration: mono Opus at 32 kb/s, resampled to 48 kHz (`--mic`, `--no-audio`)
- [x] Both tracks go in one crash-safe `screen.mkv`. Its timestamps are on the recording clock, so a step's `t_ms` is also its position in the video.
- [x] `openscrib probe` decodes every frame and reports per-track timing, gaps and size
- [x] Measured: a mostly static minute is about 1.1 MB (about 11 MB per 10 min); audio had 0 dropped packets over 60 s
- [x] The mouse cursor is not in the video yet. It will be drawn at export from `events.jsonl`, which allows smoothing and highlighting.
- [x] MP4 export (for QuickTime, PowerPoint and LMS platforms) comes with M3

### Video setup

The app needs Cisco's `openh264-2.6.0-win64.dll`, either beside the executable or in `runtime/`:

```bash
mkdir -p runtime && curl -L http://ciscobinary.openh264.org/openh264-2.6.0-win64.dll.bz2 | bzip2 -d > runtime/openh264-2.6.0-win64.dll
```

Building libopus needs CMake and a complete Visual Studio C++ toolset. If CMake can't find yours, point it at a specific install in a local `.cargo/config.toml`, for example `[env] CMAKE_GENERATOR = "Visual Studio 16 2019"`.

## Try it

```bash
cargo run -p openscrib-cli -- record --out demo.scrib
```

Click around, type something, then press **Ctrl+Shift+F12** to stop (change it with `--stop-key`, or in the editor's Settings).
Without `--out`, recordings go to `projects/` beside the executable
(`target/release/projects/` under `cargo run --release`). `info`, `probe` and
`markers` also look there, so the bare file name that `record` prints works.

```bash
cargo run -p openscrib-cli -- info demo.scrib
```

```bash
cargo run -p openscrib-cli -- markers demo.scrib --out demo-markers
```

`markers` draws the click markers onto copies of every frame and writes a
`guide.md` preview. That makes it quick to check marker accuracy.

Run `openscrib devices` to list monitor ids and microphones. Other flags: `--no-keys` records typed characters as `•` and leaves them out of
the event log, `--duration <secs>` stops automatically, and `--keep-dir` keeps
the unpacked working directory. If a recording crashes, run
`openscrib pack <dir> <out.scrib>` on the leftover `*.scrib.recording` folder.

## Layout

| Crate | Role |
| --- | --- |
| `openscrib-core` | Clock, `project.json` schema, `.scrib` pack and unpack, click markers. Has no OS dependencies. |
| `openscrib-capture` | Input hooks, screen capture, window lookup, and the `Session` step state machine |
| `openscrib-export` | Step image rendering, Markdown/HTML/PDF guides, video export, MP4 remux |
| `openscrib-process` | Model downloads, Whisper transcription, OCR, vision stage, drafting step text |
| `openscrib-vision` | Helper exe running the vision language model (llama.cpp) |
| `openscrib-cli` | The `openscrib` binary (`record`, `export`, `process`, `models`, `info`, `probe`, `markers`, `pack`, `devices`) |
| `app/` | Tauri 2 + Svelte 5 editor (`app/src-tauri` is the Rust side) |


## Licence

Copyright © 2026 Rovamp Services Pty Ltd.

Dual-licensed under MIT ([LICENSE-MIT](LICENSE-MIT)) or Apache-2.0, at your option.

# OpenSCRIB

A portable, local-first step recorder. Record once and export twice: you get a
step-by-step guide with annotated screenshots and an edited video. Capture works
from pixels and coordinates, so it also works through TeamViewer and other
remote sessions.

The design is in [docs/outline.md](docs/outline.md).

## Status: M1 (capture core, Windows)

- [x] Global mouse and keyboard hooks (`rdev`), per-monitor DPI aware
- [x] Lossless full-monitor PNG at every click, plus a pre-click frame when the mouse rested ≥500 ms (for tooltips)
- [x] One monotonic clock; every event and step in ms from the recording start
- [x] Click coordinates stored for the screen, the target window and the frame
- [x] Window title, process, frame bounds and client area of the click target
- [x] Double-click merge; typing bursts collapse into one step (`hi{Tab}there{Enter}`); shortcuts become their own steps (`Ctrl+S`)
- [x] Blank or black frame warning (capture blocked by policy)
- [x] Crash-safe working directory, packed into a `.scrib` zip on stop
- [ ] **Done-when check:** a recording through TeamViewer gives correct markers on every PNG (needs a manual run)
- [ ] Warn when the target app is elevated and we are not

## Status: M2 (video and narration)

- [x] Continuous screen video of one monitor at ≤12 fps (`--fps`, `--monitor`). Pixel-identical frames are skipped, so the video has a variable frame rate.
- [x] H.264 via Cisco's OpenH264 binary, loaded at runtime and verified by SHA-256
- [x] Microphone narration: mono Opus at 32 kb/s, resampled to 48 kHz (`--mic`, `--no-audio`)
- [x] Both tracks go in one crash-safe `screen.mkv`. Its timestamps are on the recording clock, so a step's `t_ms` is also its position in the video.
- [x] `openscrib probe` decodes every frame and reports per-track timing, gaps and size
- [x] Measured: a mostly static minute is about 1.1 MB (about 11 MB per 10 min); audio had 0 dropped packets over 60 s
- [ ] The mouse cursor is not in the video yet. It will be drawn at export from `events.jsonl`, which allows smoothing and highlighting.
- [ ] MP4 export (for QuickTime, PowerPoint and LMS platforms) comes with M3

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

Click around, type something, then press **Ctrl+Shift+F12** to stop.
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
| `openscrib-cli` | The `openscrib` binary |

The Tauri editor app arrives with M3 and builds on `core` and `capture`.

## Licence

Dual-licensed under MIT ([LICENSE-MIT](LICENSE-MIT)) or Apache-2.0, at your option.

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

## Try it

```bash
cargo run -p openscrib-cli -- record --out demo.scrib
```

Click around, type something, then press **Ctrl+Shift+F12** to stop.

```bash
cargo run -p openscrib-cli -- info demo.scrib
```

```bash
cargo run -p openscrib-cli -- markers demo.scrib --out demo-markers
```

`markers` draws the click markers onto copies of every frame and writes a
`guide.md` preview. That makes it quick to check marker accuracy.

Other flags: `--no-keys` records typed characters as `•` and leaves them out of
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

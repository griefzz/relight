# relight

A video editor and screen recorder for Windows, built with Tauri 2 (Rust) and React +
TypeScript. Cut clips together, then export at full quality or under a Discord size limit.

## Requirements

- Windows 10 or 11
- An NVIDIA GPU is optional (used for recording)

The installers include ffmpeg. Built from source, relight uses `ffmpeg` and `ffprobe` 6 or
newer from the PATH.

## Use

```sh
relight                 # open the window (or bring back the running one)
relight clip.mp4        # open with a clip on the timeline
relight trip.relight    # open a project
```

The setup `.exe` puts the `relight` command on your PATH (open a new terminal after
installing), unless you untick that on its Options page (or pass `/NOPATH` to a silent
`/S` install); updates keep the choice, and uninstalling takes it off. If you also have a
copy from `npm run install-cli`, whichever comes first on the PATH wins:
`cargo uninstall relight` leaves the installed one.

**Editing.** One track; clips play end to end. Split, trim by dragging an edge, move by
dragging, copy/cut/paste, undo/redo. Drag videos in from the media bin. Clips show
thumbnails and a waveform.

**Projects.** Saved as `.relight` files (JSON). They point to the media instead of copying
it. If files have moved, relight asks where they are.

**Export.** **Full quality** (H.264, visually lossless) or **Discord** (fits under an MB
limit, 20 by default; H.264 or AV1). Size and frame rate can be lowered, but never raised
above the first clip's.

## Recording

- **Ctrl + 5** starts and stops recording from anywhere, games included. **Record** in the
  top bar does the same. Change the hotkey in Settings (the gear, or Ctrl + ,).
- Captures the screen the mouse is on, game sound (the output device) and the microphone.
- Encodes on the GPU with NVENC; without an NVIDIA card it falls back to x264.
- Saves to `Videos\relight` and adds the recording to the timeline. The first audio track
  is the mix; separate Game and Microphone tracks follow it.
- **Balancing them:** select a recording's clips and the timeline's toolbar shows a Game and
  a Mic slider (0 to 200%; double-click one to put it back). The player and the export both
  use them.
- Closing the window leaves relight in the tray (a setting), so the hotkey keeps working.
  **Quit** is in the tray menu.
- **Hotkey does nothing?** Another program holds it. Windows gives a hotkey to only one
  program (ZoomIt uses Ctrl + 5, for example).

## Keys

| Key | Does |
| --- | --- |
| Space or K | Play / pause |
| ← / → | Back / forward a frame |
| Shift + ← / → | Back / forward a second |
| Home / End | Start / end |
| S | Split the clip at the playhead |
| F | Full screen (Esc leaves it) |
| Delete | Delete the selected clips (the rest close up) |
| Ctrl + C / X / V | Copy / cut / paste (pastes at the playhead) |
| Ctrl + Z, Ctrl + Shift + Z | Undo, redo |
| Ctrl + A, Escape | Select all, select none |
| Ctrl + I | Import videos |
| Ctrl + N / O / S | New, open, save project (Ctrl + Shift + S saves as) |
| Ctrl + E | Export |
| Ctrl + , | Settings |
| Ctrl + wheel | Zoom the timeline |

## Build from source

Needs Node 20+, stable Rust and the [Tauri prerequisites](https://tauri.app/start/prerequisites/)
for Windows (WebView2, MSVC build tools).

```sh
npm install
npm run tauri dev        # run with hot reload
npm run install-cli      # release build of relight.exe into ~/.cargo/bin (on PATH)
npm run installers       # the installers, with ffmpeg in them (downloaded first, ~90 MB)
npm test                 # the window's tests
cargo test --workspace -- --include-ignored   # from the repo root; the ignored tests need ffmpeg
cargo run --release -p relight-media --example discord_encode -- clip.mp4 [out.mp4] [--mb 20] [--av1] [--film]
```

The last command runs the Discord preset on its own, without the app. Pushes to `main`
build the Windows installers in GitHub Actions; they're attached to the workflow run as
artifacts.

### Releasing

Bump `version` in the root `Cargo.toml`, commit, and push a matching tag:

```sh
git tag v0.2.0
git push origin main v0.2.0
```

The release workflow builds the setup `.exe`, signs it, and publishes it as a GitHub
release with a `latest.json`. Installed copies look for that at startup and every few
hours, download a newer version, and offer it in the top bar. Signing needs the
`TAURI_SIGNING_PRIVATE_KEY` repository secret, whose public half is in `tauri.conf.json`.

The ffmpeg the installers ship with is a pinned build from
[BtbN/FFmpeg-Builds](https://github.com/BtbN/FFmpeg-Builds), checked against its hash;
`scripts/fetch-ffmpeg.mjs` says how to move to a newer one.

## Layout

- `src/`: the React window
- `src-tauri/`: the app (window, commands, tray, hotkey, settings)
- `src-tauri/windows/`: the installer's PATH option. `installer.nsi` is Tauri's installer
  template for the pinned `@tauri-apps/cli` with one line added, so moving to another CLI
  version means taking its template and adding that line back.
- `crates/ffmpeg`: finding and running ffmpeg and ffprobe
- `crates/media`: timeline rendering, export presets (including Discord), thumbnails and waveforms
- `crates/recorder`: screen and sound capture

## License

relight is MIT: see [LICENSE](LICENSE). The installers also include FFmpeg, which is GPL
version 3; its license and where to get its source are in the `ffmpeg` folder where relight
is installed.

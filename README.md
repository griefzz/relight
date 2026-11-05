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
thumbnails and a waveform. **×** on a video in the bin takes it out of the project, along
with its clips (undo brings them back); the file itself stays where it is.

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
- Closing the window leaves relight in the tray (a setting), so the hotkeys keep working.
  **Quit** is in the tray menu. The window is unloaded there, freeing its memory for your
  game, and comes back in a moment as you left it, unsaved changes and undo included. It
  only hides, staying loaded, while a dialog is open, an export is running or an update is
  waiting to install.
- **Hotkey does nothing?** Another program holds it. Windows gives a hotkey to only one
  program (ZoomIt uses Ctrl + 5, for example).

## Instant replay

Keeps the last stretch of the screen in memory, so you can save something after it's
happened. Nothing reaches the disk until you save, unless you turn on bounce to disk.

| Hotkey | Does |
| --- | --- |
| Alt + F6 | Start it |
| Alt + F7 | Stop it, and let go of what it holds |
| Alt + F8 | Save the last 30 seconds |

- The hotkeys work from anywhere, and are in Settings along with the length (30 seconds by
  default, up to 5 minutes). **Replay** in the top bar and the tray menu do the same.
- It uses the recording settings for the screen, frame rate and game sound. The screen the
  mouse is on means the one it's on when replay starts. The microphone is off unless you
  turn it on under Instant replay in Settings.
- Saving takes a couple of seconds, as the newest frames are still being encoded. The window
  stays out of the way of your game, and the clip goes in `Videos\relight` and the media
  bin. To hear that it's on its way, turn on the sound alert in Settings: a chime when you
  save, and a lower one if it failed. A clip is the length asked for and up to 2 seconds
  more, as it has to start on a keyframe.
- Every clip you save is its own file straight away, so saving many costs no extra memory.
  What memory it uses is the stretch it's holding, ready to save: up to about 220 MB at 30
  seconds, and 2 GB at 5 minutes.
- **Bounce to disk**, under Instant replay in Settings, frees most of that for your game on
  long sessions: the picture goes to temporary files as it's captured, using as much disk
  space as it would have memory. The sound (a small part of it) stays in memory, and if the
  disk can't take the picture, it's held in memory instead and saving a clip says so.
- Without an NVIDIA card it encodes with x264 on the CPU, all the time it's on, which can
  slow a game down; relight says so when it starts.
- Starting a recording stops replay and clears what it holds; a fresh one starts when the
  recording's done.
- If the screen capture stops (a game changing the display mode, the screen locking) it
  starts again by itself, and a clip reaches back across that as usual, holding on the last
  frame for the moment there was no picture. It can't reach back past the screen changing
  size, or the PC sleeping: a clip only goes back to then, and says so if the size changed.

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
release with a `latest.json`. Installed copies look for that when the window opens and
every few hours it's open, download a newer version, and offer it in the top bar. Signing needs the
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
- `crates/recorder`: screen and sound capture, for recordings and instant replay

## License

relight is MIT: see [LICENSE](LICENSE). The installers also include FFmpeg, which is GPL
version 3; its license and where to get its source are in the `ffmpeg` folder where relight
is installed.

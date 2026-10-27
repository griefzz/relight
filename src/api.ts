// The Rust side's commands and events, typed.

import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export interface VideoInfo {
  codec: string;
  profile: string | null;
  pixFmt: string | null;
  width: number;
  height: number;
  fps: number;
}

export interface AudioInfo {
  codec: string;
  bitRate: number | null;
}

export interface MediaInfo {
  path: string;
  duration: number;
  size: number;
  video: VideoInfo | null;
  audio: AudioInfo | null;
}

export type Tune = "animation" | "film";

export type Preset =
  | { kind: "full" }
  | { kind: "discord"; targetMb: number; av1: boolean; tune: Tune };

export type ExportReport =
  | { type: "stage"; label: string }
  | { type: "progress"; fraction: number }
  | { type: "note"; text: string };

export interface Exported {
  path: string;
  bytes: number;
}

/** The error a cancelled export ends with. */
export const CANCELLED = "Cancelled";

/** The clip or project named on the command line, the first time it's asked for. */
export const initialFile = () => invoke<string | null>("initial_file");

/** Reads a clip's streams and lets the player load it. */
export const openMedia = (path: string) => invoke<MediaInfo>("open_media", { path });

/** A piece of the timeline: `start` to `end` seconds of the media at `path`. */
export interface Segment {
  path: string;
  start: number;
  end: number;
}

/** The exported picture, where it differs from the first clip's (never above it). */
export interface Picture {
  /** The shorter side in pixels: 720 for 1280×720 or for 720×1280. */
  shortSide: number | null;
  fps: number | null;
}

export const exportTimeline = (segments: Segment[], picture: Picture, output: string, preset: Preset) =>
  invoke<Exported>("export", { segments, picture, output, preset });

export interface Previews {
  /** An image of `tiles` thumbnails side by side, one every `step` seconds; null with no picture. */
  strip: string | null;
  tiles: number;
  tileWidth: number;
  tileHeight: number;
  step: number;
  /** Whether `previewPeaks` has anything to give. */
  sound: boolean;
  peaksPerSecond: number;
}

/** The clip's previews, made first if they aren't cached. */
export const getPreviews = (path: string) => invoke<Previews>("previews", { path });

/** How loud the clip's sound is: a byte, 0 to 255, every 1 / `peaksPerSecond` seconds. */
export const getPreviewPeaks = async (path: string) =>
  new Uint8Array(await invoke<ArrayBuffer>("preview_peaks", { path }));

export const saveProjectFile = (path: string, contents: string) =>
  invoke<void>("save_project", { path, contents });

export const loadProjectFile = (path: string) => invoke<string>("load_project", { path });

export const cancelExport = () => invoke<void>("cancel_export");

export const isExporting = () => invoke<boolean>("is_exporting");

export const onExportReport = (handle: (report: ExportReport) => void) =>
  listen<ExportReport>("export", (event) => handle(event.payload));

// Recording

export interface Settings {
  /** Starts and stops recording from anywhere: "Ctrl+5", "Alt+Shift+R", "F9". */
  hotkey: string;
  /** The screen recorded, by number; null for the one the mouse is on. */
  screen: number | null;
  fps: number;
  gameSound: boolean;
  /** The output device recorded; null for the default. */
  gameDevice: string | null;
  microphone: boolean;
  micDevice: string | null;
  /** Where recordings go; null for a relight folder in Videos. */
  folder: string | null;
  /** Closing the window leaves relight in the tray, so the hotkey still works. */
  keepRunning: boolean;
}

export const getSettings = () => invoke<Settings>("get_settings");

/** Saves the settings; fails, changing nothing, if a new hotkey can't be had. */
export const setSettings = (next: Settings) => invoke<Settings>("set_settings", { next });

/** Lets go of the hotkey while a new one is typed in (so it doesn't start recording), and takes it back. */
export const pauseHotkey = (paused: boolean) => invoke<void>("pause_hotkey", { paused });

/** Whether the hotkey is relight's: not if another program had it first. */
export const hotkeyWorking = () => invoke<boolean>("hotkey_working");

export interface Screen {
  index: number;
  x: number;
  y: number;
  width: number;
  height: number;
  primary: boolean;
}

export interface Sources {
  screens: Screen[];
  sound: { outputs: string[]; inputs: string[]; defaultOutput: string | null; defaultInput: string | null };
}

export const recordingSources = () => invoke<Sources>("recording_sources");

export type RecorderStatus =
  | { state: "idle" }
  | { state: "starting" }
  | { state: "recording"; since: number }
  | { state: "saving" };

export const recorderStatus = () => invoke<RecorderStatus>("recorder_status");

export const toggleRecording = () => invoke<void>("toggle_recording");

export interface Recorded {
  path: string;
  /** Sources that couldn't be recorded; the recording went ahead without them. */
  warnings: string[];
}

export const onRecorder = (handle: (status: RecorderStatus) => void) =>
  listen<RecorderStatus>("recorder", (event) => handle(event.payload));

export const onRecorded = (handle: (recorded: Recorded) => void) =>
  listen<Recorded>("recorded", (event) => handle(event.payload));

export const onRecorderError = (handle: (error: string) => void) =>
  listen<string>("recorder-error", (event) => handle(event.payload));

// The app

/** A file to open, from `relight file` typed while relight was already running. */
export const onOpenFile = (handle: (path: string) => void) =>
  listen<string>("open-file", (event) => handle(event.payload));

/** Quit, picked from the tray: the window checks for unsaved work first. */
export const onQuitRequested = (handle: () => void) => listen("quit-requested", () => handle());

/** Saves any recording going, then quits. */
export const quitApp = () => invoke<void>("quit");

/** Something that went wrong starting up (the hotkey taken), the first time it's asked. */
export const takeNotice = () => invoke<string | null>("take_notice");

/** Whether this copy updates itself: only released ones do. */
export const updatesOn = () => invoke<boolean>("updates_on");

/** A URL the player can stream a local file from. */
export const mediaUrl = (path: string) => convertFileSrc(path);

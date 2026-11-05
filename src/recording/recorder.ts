// The recorder's state as the window sees it, and the settings it records with.

import { create } from "zustand";
import {
  getSettings,
  onRecorder,
  onReplay,
  recorderStatus,
  replayStatus,
  setSettings,
  type RecorderStatus,
  type ReplayStatus,
  type Settings,
} from "../api";

export const useRecording = create<{ status: RecorderStatus; replay: ReplayStatus; settings: Settings | null }>(() => ({
  status: { state: "idle" },
  replay: { state: "off" },
  settings: null,
}));

/** Gets the recorder's state and settings, and follows the recorder from then on. */
export async function followRecorder() {
  const stops = await Promise.all([
    onRecorder((status) => useRecording.setState({ status })),
    onReplay((replay) => useRecording.setState({ replay })),
  ]);
  const [status, replay, settings] = await Promise.all([recorderStatus(), replayStatus(), getSettings()]);
  useRecording.setState({ status, replay, settings });
  return () => stops.forEach((stop) => stop());
}

/** Saves `next`; if it can't be (a hotkey that's taken), nothing changes and it throws. */
export async function changeSettings(next: Settings) {
  useRecording.setState({ settings: await setSettings(next) });
}

/** The longest a recording's picture can take in memory, in megabytes a second. */
const PICTURE_MB_A_SECOND = 6.25;
/** Stereo game sound and a mono microphone, as raw samples at 48 kHz. */
const SOUND_MB_A_SECOND = 0.4;
const MICROPHONE_MB_A_SECOND = 0.2;
/** How much more than asked for is held, to start a save on a keyframe. */
const EXTRA_SECONDS = 3;
/** How much picture arrives at a time. Bouncing to disk, that's all of it that's in memory. */
const CHUNK_SECONDS = 2;

/** The most memory instant replay should use, in megabytes. Bouncing to disk only takes the picture out. */
export function replayMemory(seconds: number, gameSound: boolean, microphone: boolean, bounce: boolean) {
  const held = seconds + EXTRA_SECONDS;
  const sound = (gameSound ? SOUND_MB_A_SECOND : 0) + (microphone ? MICROPHONE_MB_A_SECOND : 0);
  return Math.round((bounce ? CHUNK_SECONDS : held) * PICTURE_MB_A_SECOND + held * sound);
}

/** The most temporary disk space instant replay should use while bouncing to disk, in megabytes. */
export function replayDisk(seconds: number) {
  return Math.round((seconds + EXTRA_SECONDS) * PICTURE_MB_A_SECOND);
}

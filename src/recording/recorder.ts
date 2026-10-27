// The recorder's state as the window sees it, and the settings it records with.

import { create } from "zustand";
import { getSettings, onRecorder, recorderStatus, setSettings, type RecorderStatus, type Settings } from "../api";

export const useRecording = create<{ status: RecorderStatus; settings: Settings | null }>(() => ({
  status: { state: "idle" },
  settings: null,
}));

/** Gets the recorder's state and settings, and follows the recorder from then on. */
export async function followRecorder() {
  const stop = await onRecorder((status) => useRecording.setState({ status }));
  const [status, settings] = await Promise.all([recorderStatus(), getSettings()]);
  useRecording.setState({ status, settings });
  return stop;
}

/** Saves `next`; if it can't be (a hotkey that's taken), nothing changes and it throws. */
export async function changeSettings(next: Settings) {
  useRecording.setState({ settings: await setSettings(next) });
}

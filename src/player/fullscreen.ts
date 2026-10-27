// Full screen: the viewer filling the window, and the window filling the screen.

import { getCurrentWindow } from "@tauri-apps/api/window";
import { create } from "zustand";

export const useFullscreen = create<{ on: boolean }>(() => ({ on: false }));

export async function setFullscreen(on: boolean) {
  if (useFullscreen.getState().on === on) return;
  useFullscreen.setState({ on });
  await getCurrentWindow()
    .setFullscreen(on)
    .catch(() => {});
}

export const toggleFullscreen = () => setFullscreen(!useFullscreen.getState().on);

/** Notices the window leaving full screen some other way (Windows + Down, say). */
export function followWindow(): () => void {
  const window = getCurrentWindow();
  const stop = window.onResized(async () => {
    if (useFullscreen.getState().on && !(await window.isFullscreen())) useFullscreen.setState({ on: false });
  });
  return () => {
    stop.then((unlisten) => unlisten());
  };
}

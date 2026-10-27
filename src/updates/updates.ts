// Updates: looked for when relight starts and every few hours after (it can sit in the
// tray for days), downloaded in the background, and installed when the user says.

import { relaunch } from "@tauri-apps/plugin-process";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { create } from "zustand";

const EVERY = 6 * 60 * 60 * 1000;

/** A newer version, downloaded and ready to install. */
export const useUpdate = create<{ ready: Update | null }>(() => ({ ready: null }));

let looking = false;

async function look() {
  if (looking || useUpdate.getState().ready) return;
  looking = true;
  try {
    const update = await check();
    if (update) {
      await update.download();
      useUpdate.setState({ ready: update });
    }
  } catch (e) {
    // Offline, say: it's looked for again later.
    console.warn("Couldn't look for an update:", e);
  } finally {
    looking = false;
  }
}

/** Starts looking for updates (not in development); returns what stops it. */
export function followUpdates(): () => void {
  if (import.meta.env.DEV) return () => {};
  look();
  const timer = setInterval(look, EVERY);
  return () => clearInterval(timer);
}

/** Installs the update and starts the new version. On Windows the installer closes relight itself. */
export async function installUpdate() {
  const update = useUpdate.getState().ready;
  if (!update) return;
  await update.install();
  await relaunch();
}

// Thumbnails and waveforms for the timeline's clips, made in the background, one file
// at a time, and kept here by path once ready.

import { create } from "zustand";
import { getPreviewPeaks, getPreviews, mediaUrl, type Previews } from "../api";

export interface Preview extends Previews {
  stripUrl: string | null;
  peaks: Uint8Array | null;
  /** For a recording, the peaks of each of its sound sources. */
  sourcePeaks: Uint8Array[];
}

export const usePreviews = create<Record<string, Preview>>(() => ({}));

const asked = new Set<string>();
const waiting: string[] = [];
let working = false;

/** Starts making previews for any of `paths` not asked for before. */
export function requestPreviews(paths: string[]) {
  for (const path of paths) {
    if (asked.has(path)) continue;
    asked.add(path);
    waiting.push(path);
  }
  if (!working) work();
}

async function work() {
  working = true;
  for (let path = waiting.shift(); path; path = waiting.shift()) {
    try {
      const previews = await getPreviews(path);
      const peaks = previews.sound ? await getPreviewPeaks(path) : null;
      const sourcePeaks = await Promise.all(previews.sources.map((_, n) => getPreviewPeaks(path, n)));
      const stripUrl = previews.strip ? mediaUrl(previews.strip) : null;
      usePreviews.setState({ [path]: { ...previews, stripUrl, peaks, sourcePeaks } });
    } catch (e) {
      // The clip just shows its colour, as before.
      console.warn(`No previews for ${path}:`, e);
    }
  }
  working = false;
}

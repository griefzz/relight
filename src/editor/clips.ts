// The timeline's clips: pieces cut from media files, played end to end. These functions
// never change the clips they're given; each edit returns a new list.

import type { MediaInfo } from "../api";

export interface Clip {
  id: string;
  /** The media file it's cut from. */
  path: string;
  /** Where it starts and ends in that file, in seconds. */
  start: number;
  end: number;
  /** For a recording, how loud each of its sound sources is (1 as recorded); none for as recorded. */
  levels?: number[];
}

export type Edge = "start" | "end";

export const newId = () => crypto.randomUUID();

export const clipLength = (clip: Clip) => clip.end - clip.start;

/** The sound sources of a relight recording ("Game", "Microphone"), the tracks after its mix; null for any other file. */
export function sourcesOf(info: MediaInfo | undefined): string[] | null {
  const tracks = info?.tracks ?? [];
  return tracks.length >= 3 && tracks[0] === "Mix" ? tracks.slice(1) : null;
}

/** The most a source can be turned up: twice as loud as recorded. */
export const MOST_LEVEL = 2;

/** `clip` with source `n` at `level`, given its file's `sources`. Levels back at 1 all round are dropped. */
export function withLevel(clip: Clip, sources: string[], n: number, level: number): Clip {
  const levels = (clip.levels ?? sources.map(() => 1)).map((old, i) => (i === n ? clamp(level, 0, MOST_LEVEL) : old));
  const { levels: _, ...rest } = clip;
  return levels.every((l) => l === 1) ? rest : { ...rest, levels };
}

export const totalLength = (clips: Clip[]) => clips.reduce((sum, clip) => sum + clipLength(clip), 0);

/** Where each clip starts on the timeline. */
export function clipStarts(clips: Clip[]): number[] {
  const starts = [];
  let t = 0;
  for (const clip of clips) {
    starts.push(t);
    t += clipLength(clip);
  }
  return starts;
}

/**
 * Times this close count as the same: a clip's start worked out one way (the total less
 * the last clip) can come out a hair short of the same point added up another way.
 */
const SAME_TIME = 1e-6;

/** The clip at timeline time `t` and how far into it; past the end, the end of the last. */
export function locate(clips: Clip[], t: number): { index: number; offset: number } | null {
  if (!clips.length) return null;
  let start = 0;
  for (let i = 0; i < clips.length; i++) {
    const length = clipLength(clips[i]);
    if (t < start + length - SAME_TIME) return { index: i, offset: Math.max(0, t - start) };
    start += length;
  }
  const last = clips.length - 1;
  return { index: last, offset: clipLength(clips[last]) };
}

/** The start of the frame showing at `time` seconds into media at `fps`. */
export const frameStart = (time: number, fps: number) => Math.floor(time * fps + 1e-6) / fps;

const nearestFrame = (time: number, fps: number) => Math.round(time * fps) / fps;

export const clamp = (x: number, min: number, max: number) => Math.min(Math.max(x, min), max);

/**
 * Cuts the clip at timeline time `t` in two, at the start of the frame showing there.
 * Also says where something inserted at `t` goes.
 */
export function cutAt(clips: Clip[], t: number, fpsOf: (path: string) => number): { clips: Clip[]; index: number } {
  const at = locate(clips, t);
  if (!at) return { clips, index: 0 };
  const clip = clips[at.index];
  const fps = fpsOf(clip.path);
  const cut = frameStart(clip.start + at.offset, fps);
  if (cut - clip.start < 0.5 / fps) return { clips, index: at.index };
  if (clip.end - cut < 0.5 / fps) return { clips, index: at.index + 1 };
  const left = { ...clip, end: cut };
  const right = { ...clip, id: newId(), start: cut };
  return { clips: [...clips.slice(0, at.index), left, right, ...clips.slice(at.index + 1)], index: at.index + 1 };
}

export function insertAt(clips: Clip[], index: number, added: Clip[]): Clip[] {
  return [...clips.slice(0, index), ...added, ...clips.slice(index)];
}

/** Moves clip `id` to just before the clip now at `index` (the end, past the last). */
export function moveTo(clips: Clip[], id: string, index: number): Clip[] {
  const from = clips.findIndex((clip) => clip.id === id);
  if (from < 0 || index === from || index === from + 1) return clips;
  const rest = clips.filter((clip) => clip.id !== id);
  return insertAt(rest, index > from ? index - 1 : index, [clips[from]]);
}

/** Moves one edge of clip `id` to `time` in its media, on a frame, keeping it a frame long at least. */
export function trimTo(clips: Clip[], id: string, edge: Edge, time: number, media: MediaInfo): Clip[] {
  const fps = media.video?.fps || 30;
  const shortest = 1 / fps;
  return clips.map((clip) => {
    if (clip.id !== id) return clip;
    if (edge === "start") return { ...clip, start: clamp(nearestFrame(time, fps), 0, clip.end - shortest) };
    const end = time >= media.duration - 0.5 / fps ? media.duration : nearestFrame(time, fps);
    return { ...clip, end: clamp(end, clip.start + shortest, media.duration) };
  });
}

export function sameClips(a: Clip[], b: Clip[]): boolean {
  const sameLevels = (x?: number[], y?: number[]) => (x ?? []).join() === (y ?? []).join();
  return (
    a.length === b.length &&
    a.every(
      (clip, i) =>
        clip.id === b[i].id && clip.start === b[i].start && clip.end === b[i].end && sameLevels(clip.levels, b[i].levels),
    )
  );
}

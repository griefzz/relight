// The editor's state, and every change to it.

import { create } from "zustand";
import type { MediaInfo } from "../api";
import {
  type Clip,
  type Edge,
  clipStarts,
  cutAt,
  frameStart,
  insertAt,
  locate,
  moveTo,
  newId,
  sameClips,
  sourcesOf,
  totalLength,
  trimTo,
  withLevel,
} from "./clips";

const HISTORY = 200;

interface Editor {
  /** The media bin, by path, in the order it was added. */
  media: Record<string, MediaInfo>;
  clips: Clip[];
  past: Clip[][];
  future: Clip[][];
  /** The clips as they were when a drag began, so the whole drag undoes in one step. */
  dragStart: Clip[] | null;
  selection: string[];
  clipboard: Clip[];
  playhead: number;
  /** Goes up when the playhead is moved by hand, so the player knows to follow. */
  seeks: number;
  playing: boolean;
  projectPath: string | null;
  dirty: boolean;
}

const blank: Editor = {
  media: {},
  clips: [],
  past: [],
  future: [],
  dragStart: null,
  selection: [],
  clipboard: [],
  playhead: 0,
  seeks: 0,
  playing: false,
  projectPath: null,
  dirty: false,
};

export const useEditor = create<Editor>(() => blank);

const get = useEditor.getState;
const set = useEditor.setState;

export const fpsOf = (path: string) => get().media[path]?.video?.fps || 30;

/** Makes `clips` the timeline as one step that can be undone. */
function commit(clips: Clip[], then: Partial<Editor> = {}) {
  const s = get();
  if (clips === s.clips) return;
  set({
    clips,
    past: [...s.past, s.clips].slice(-HISTORY),
    future: [],
    dirty: true,
    playhead: Math.min(s.playhead, totalLength(clips)),
    ...then,
  });
}

// Media

export function addMedia(info: MediaInfo) {
  set((s) => ({ media: { ...s.media, [info.path]: info }, dirty: true }));
}

const wholeClip = (path: string): Clip | null => {
  const info = get().media[path];
  return info ? { id: newId(), path, start: 0, end: info.duration } : null;
};

export function appendMedia(path: string) {
  const clip = wholeClip(path);
  if (clip) commit([...get().clips, clip]);
}

/** Puts `added` in at the playhead, cutting the clip there, and moves the playhead past them. */
function insertAtPlayhead(added: Clip[]) {
  const { clips, index } = cutAt(get().clips, get().playhead, fpsOf);
  const next = insertAt(clips, index, added);
  commit(next, {
    selection: added.map((clip) => clip.id),
    playhead: clipStarts(next)[index] + totalLength(added),
    seeks: get().seeks + 1,
  });
}

export function insertMedia(path: string) {
  const clip = wholeClip(path);
  if (clip) insertAtPlayhead([clip]);
}

/** Puts a whole clip of `path` just before the clip now at `index` (the end, past the last). */
export function insertMediaAt(path: string, index: number) {
  const clip = wholeClip(path);
  if (clip) commit(insertAt(get().clips, index, [clip]), { selection: [clip.id] });
}

// Editing

export function split() {
  commit(cutAt(get().clips, get().playhead, fpsOf).clips);
}

export function deleteSelected() {
  const { clips, selection } = get();
  if (selection.length) commit(clips.filter((clip) => !selection.includes(clip.id)), { selection: [] });
}

export function copySelection() {
  const { clips, selection } = get();
  if (selection.length) set({ clipboard: clips.filter((clip) => selection.includes(clip.id)) });
}

export function cutSelection() {
  copySelection();
  deleteSelected();
}

export function paste() {
  const { clipboard } = get();
  if (clipboard.length) insertAtPlayhead(clipboard.map((clip) => ({ ...clip, id: newId() })));
}

export function moveClip(id: string, index: number) {
  commit(moveTo(get().clips, id, index));
}

export function beginDrag() {
  set({ dragStart: get().clips });
}

/** Moves a clip's edge during a drag; `endDrag` makes the whole drag one undo step. */
export function trimClip(id: string, edge: Edge, time: number) {
  const { clips, media } = get();
  const clip = clips.find((c) => c.id === id);
  if (clip) set({ clips: trimTo(clips, id, edge, time, media[clip.path]) });
}

/**
 * Sets how loud source `n` is in the selected clips that are recordings; during a drag
 * (`beginDrag` to `endDrag`), as one undo step.
 */
export function changeLevel(n: number, level: number) {
  const { clips, selection, media, dragStart } = get();
  const next = clips.map((clip) => {
    const sources = selection.includes(clip.id) ? sourcesOf(media[clip.path]) : null;
    return sources && n < sources.length ? withLevel(clip, sources, n, level) : clip;
  });
  if (sameClips(next, clips)) return;
  if (dragStart) set({ clips: next });
  else commit(next);
}

export function endDrag() {
  const { dragStart, clips, past, playhead } = get();
  if (dragStart && !sameClips(dragStart, clips)) {
    set({
      past: [...past, dragStart].slice(-HISTORY),
      future: [],
      dirty: true,
      playhead: Math.min(playhead, totalLength(clips)),
    });
  }
  set({ dragStart: null });
}

function restore(clips: Clip[], past: Clip[][], future: Clip[][]) {
  const s = get();
  set({
    clips,
    past,
    future,
    dirty: true,
    selection: s.selection.filter((id) => clips.some((clip) => clip.id === id)),
    playhead: Math.min(s.playhead, totalLength(clips)),
  });
}

export function undo() {
  const { past, future, clips } = get();
  const previous = past[past.length - 1];
  if (previous) restore(previous, past.slice(0, -1), [clips, ...future]);
}

export function redo() {
  const { past, future, clips } = get();
  const [next, ...rest] = future;
  if (next) restore(next, [...past, clips], rest);
}

// Selection

export function select(id: string, toggle = false) {
  set((s) => ({
    selection: !toggle ? [id] : s.selection.includes(id) ? s.selection.filter((x) => x !== id) : [...s.selection, id],
  }));
}

export const selectAll = () => set((s) => ({ selection: s.clips.map((clip) => clip.id) }));

export const clearSelection = () => set({ selection: [] });

// Playback

export function seek(t: number) {
  const total = totalLength(get().clips);
  set((s) => ({ playhead: Math.min(Math.max(t, 0), total), seeks: s.seeks + 1 }));
}

/** The player reporting where it's got to. */
export const playerTime = (t: number) => set({ playhead: t });

export const setPlaying = (playing: boolean) => set({ playing });

export function togglePlay() {
  const { clips, playing, playhead } = get();
  if (!clips.length) return;
  if (!playing && playhead >= totalLength(clips) - 1e-3) seek(0);
  set({ playing: !playing });
}

/** Pauses and moves `frames` frames, landing mid-frame so rounding can't show the one before. */
export function stepFrames(frames: number) {
  const { clips, playhead } = get();
  const here = locate(clips, playhead);
  if (!here) return;
  setPlaying(false);
  const target = Math.min(Math.max(playhead + frames / fpsOf(clips[here.index].path), 0), totalLength(clips));
  const there = locate(clips, target)!;
  const clip = clips[there.index];
  const fps = fpsOf(clip.path);
  const time = Math.min(frameStart(clip.start + there.offset, fps) + 0.5 / fps, clip.end);
  seek(clipStarts(clips)[there.index] + time - clip.start);
}

// Projects

export const newProject = () => set({ ...blank, seeks: get().seeks + 1 });

/** Opens a project; `changed` when it isn't as saved (its files were found elsewhere). */
export function loadProject(media: MediaInfo[], clips: Clip[], path: string, changed = false) {
  set({
    ...blank,
    media: Object.fromEntries(media.map((info) => [info.path, info])),
    clips,
    projectPath: path,
    dirty: changed,
    seeks: get().seeks + 1,
  });
}

export const markSaved = (path: string) => set({ projectPath: path, dirty: false });

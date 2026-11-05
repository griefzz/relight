import { beforeEach, describe, expect, it } from "vitest";
import type { MediaInfo } from "../api";
import {
  addMedia,
  addSavedReplay,
  appendMedia,
  clipsOf,
  copySelection,
  markSaved,
  newProject,
  paste,
  redo,
  removeMedia,
  restoreEditor,
  seek,
  selectAll,
  setPlaying,
  split,
  stashEditor,
  undo,
  useEditor,
} from "./store";

const video = (path: string, duration = 10): MediaInfo => ({
  path,
  duration,
  size: 1000,
  video: { codec: "h264", profile: null, pixFmt: null, width: 1920, height: 1080, fps: 60 },
  audio: null,
  tracks: [],
});

const state = useEditor.getState;
const binned = () => Object.keys(state().media);
const onTimeline = () => state().clips.map((clip) => clip.path);

beforeEach(() => {
  newProject();
  useEditor.setState({ dirty: false });
});

describe("taking a video out of the bin", () => {
  it("removes one not on the timeline, and touches nothing else", () => {
    addMedia(video("a.mp4"));
    addMedia(video("b.mp4"));
    appendMedia("b.mp4");
    useEditor.setState({ dirty: false });

    removeMedia("a.mp4");

    expect(binned()).toEqual(["b.mp4"]);
    expect(onTimeline()).toEqual(["b.mp4"]);
    expect(state().dirty).toBe(true);
    expect(state().past.length).toBe(1);
  });

  it("takes its clips off the timeline, and the rest close up", () => {
    addMedia(video("a.mp4", 5));
    addMedia(video("b.mp4", 7));
    appendMedia("a.mp4");
    appendMedia("b.mp4");
    appendMedia("a.mp4");
    expect(clipsOf("a.mp4")).toBe(2);

    removeMedia("a.mp4");

    expect(binned()).toEqual(["b.mp4"]);
    expect(onTimeline()).toEqual(["b.mp4"]);
    expect(clipsOf("a.mp4")).toBe(0);
  });

  it("forgets its clips in the selection and clipboard, so they can't be pasted back", () => {
    addMedia(video("a.mp4"));
    addMedia(video("b.mp4"));
    appendMedia("a.mp4");
    appendMedia("b.mp4");
    selectAll();
    copySelection();

    removeMedia("a.mp4");

    expect(state().selection).toHaveLength(1);
    expect(state().clipboard.map((clip) => clip.path)).toEqual(["b.mp4"]);
    paste();
    expect(onTimeline()).toEqual(["b.mp4", "b.mp4"]);
  });

  it("comes back with its clips on undo", () => {
    addMedia(video("a.mp4"));
    addMedia(video("b.mp4"));
    appendMedia("a.mp4");
    appendMedia("b.mp4");

    removeMedia("a.mp4");
    undo();

    expect(onTimeline()).toEqual(["a.mp4", "b.mp4"]);
    expect(state().media["a.mp4"]).toBeDefined();

    // Redo takes the clips off again; the video may stay in the bin, but nothing's broken.
    redo();
    expect(onTimeline()).toEqual(["b.mp4"]);
  });

  it("comes back when an older edit is undone back to clips of it", () => {
    addMedia(video("a.mp4"));
    appendMedia("a.mp4");
    removeMedia("a.mp4");
    addMedia(video("b.mp4"));
    appendMedia("b.mp4");

    undo();
    expect(onTimeline()).toEqual([]);
    undo();
    expect(onTimeline()).toEqual(["a.mp4"]);
    expect(state().media["a.mp4"]).toBeDefined();
  });

  it("leaves nothing to bring back in a new project", () => {
    addMedia(video("a.mp4"));
    removeMedia("a.mp4");
    newProject();
    expect(state().removedMedia).toEqual({});
  });

  it("does nothing for a video that isn't in the bin", () => {
    addMedia(video("a.mp4"));
    useEditor.setState({ dirty: false });
    removeMedia("nowhere.mp4");
    expect(binned()).toEqual(["a.mp4"]);
    expect(state().dirty).toBe(false);
  });
});

describe("the window being unloaded to the tray and coming back", () => {
  /** An editor with something of everything in it. */
  function inUse() {
    addMedia(video("a.mp4"));
    addMedia(video("b.mp4"));
    addMedia(video("gone.mp4"));
    appendMedia("a.mp4");
    appendMedia("b.mp4");
    appendMedia("gone.mp4");
    removeMedia("gone.mp4");
    seek(4);
    split();
    selectAll();
    copySelection();
    markSaved("trip.relight");
    seek(12);
    split();
    return JSON.parse(JSON.stringify(state()));
  }

  it("comes back as it was: clips, bin, undo history, clipboard, selection, project and unsaved changes", () => {
    const before = inUse();
    const stash = stashEditor();
    newProject();
    expect(state().clips).toEqual([]);

    restoreEditor(stash);

    const { seeks, ...after } = JSON.parse(JSON.stringify(state()));
    const { seeks: seeksBefore, ...rest } = before;
    expect(after).toEqual(rest);
    expect(seeks).toBeGreaterThan(seeksBefore - 1);
    expect(state().dirty).toBe(true);
    expect(state().projectPath).toBe("trip.relight");
  });

  it("can undo edits made before it went", () => {
    inUse();
    const clipsBefore = state().clips.length;
    restoreEditor(stashEditor());
    undo();
    expect(state().clips.length).toBe(clipsBefore - 1);
    redo();
    expect(state().clips.length).toBe(clipsBefore);
  });

  it("can undo back to a video taken out of the bin before it went", () => {
    inUse();
    restoreEditor(stashEditor());
    undo();
    undo();
    undo();
    expect(onTimeline()).toContain("gone.mp4");
    expect(state().media["gone.mp4"]).toBeDefined();
  });

  it("isn't playing when it comes back, or partway through a drag", () => {
    inUse();
    setPlaying(true);
    useEditor.setState({ dragStart: state().clips });
    restoreEditor(stashEditor());
    expect(state().playing).toBe(false);
    expect(state().dragStart).toBeNull();
  });

  it("has the player move to where it was", () => {
    inUse();
    const stash = stashEditor();
    const seeks = state().seeks;
    restoreEditor(stash);
    expect(state().seeks).toBe(seeks + 1);
    expect(state().playhead).toBe(12);
  });
});

describe("a replay saved in the background", () => {
  it("goes in the bin, not on the timeline, and isn't an unsaved change", () => {
    addSavedReplay(video("replay.mp4"));
    expect(binned()).toEqual(["replay.mp4"]);
    expect(onTimeline()).toEqual([]);
    expect(state().dirty).toBe(false);
  });
});

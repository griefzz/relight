// Plays a timeline on two video elements: one shows the current clip while the other
// waits on the next, cued up, so a cut to another file doesn't stall.

import { mediaUrl } from "../api";
import { type Clip, clipStarts, locate, totalLength } from "../editor/clips";

/** Just past a frame's start, so float rounding can't show the frame before. */
const NUDGE = 0.001;

/**
 * How early to leave a clip: the end is checked once a screen refresh, so this keeps
 * the frame after it from showing (half a frame at 60 fps).
 */
const EARLY = 0.008;

export interface EngineEvents {
  /** Where playback has got to on the timeline. */
  time: (t: number) => void;
  ended: () => void;
  /** The clip showing can't be played (its path), or can again (null). */
  unplayable: (path: string | null) => void;
}

export class Engine {
  private front = 0;
  private clips: Clip[] = [];
  private starts: number[] = [];
  private index = -1;
  private playing = false;
  private frame = 0;
  private listeners = new AbortController();

  constructor(
    private videos: [HTMLVideoElement, HTMLVideoElement],
    private on: EngineEvents,
  ) {
    const signal = this.listeners.signal;
    for (const video of videos) {
      const showing = () => video === this.videos[this.front];
      video.addEventListener("error", () => showing() && on.unplayable(video.dataset.path ?? null), { signal });
      video.addEventListener("loadeddata", () => showing() && on.unplayable(null), { signal });
    }
  }

  setClips(clips: Clip[]) {
    this.clips = clips;
    this.starts = clipStarts(clips);
  }

  /** Shows timeline time `t`, and carries on playing from there if it was. */
  seek(t: number) {
    const at = locate(this.clips, t);
    this.index = at ? at.index : -1;
    if (!at) {
      this.pause();
      this.show();
      return;
    }
    const clip = this.clips[at.index];
    const video = this.videos[this.front];
    const time = clip.start + at.offset;
    // Mid-play edits re-seek to where it already is; a seek there would only stutter.
    const already = this.playing && video.dataset.path === clip.path && Math.abs(video.currentTime - time) < 0.05;
    if (!already) cue(video, clip, time);
    if (this.playing) video.play().catch(() => {});
    this.cueNext();
    this.show();
  }

  play() {
    if (this.playing || this.index < 0) return;
    this.playing = true;
    this.videos[this.front].play().catch(() => {});
    this.frame = requestAnimationFrame(this.tick);
  }

  pause() {
    this.playing = false;
    cancelAnimationFrame(this.frame);
    for (const video of this.videos) video.pause();
  }

  destroy() {
    this.pause();
    this.listeners.abort();
  }

  private tick = () => {
    if (!this.playing) return;
    const clip = this.clips[this.index];
    const video = this.videos[this.front];
    if (video.ended || video.currentTime >= clip.end - EARLY) this.advance();
    else this.on.time(this.starts[this.index] + video.currentTime - clip.start);
    if (this.playing) this.frame = requestAnimationFrame(this.tick);
  };

  private advance() {
    const clip = this.clips[this.index];
    const next = this.clips[this.index + 1];
    if (!next) {
      this.pause();
      this.on.time(totalLength(this.clips));
      this.on.ended();
      return;
    }
    this.index++;
    // The next piece carries straight on in the same file (as after a split): keep going.
    if (next.path === clip.path && Math.abs(next.start - clip.end) < 0.002) {
      this.cueNext();
      return;
    }
    const back = this.videos[1 - this.front];
    if (back.dataset.path !== next.path || Math.abs(back.currentTime - next.start - NUDGE) > 0.05) {
      cue(back, next, next.start);
    }
    back.play().catch(() => {});
    this.videos[this.front].pause();
    this.front = 1 - this.front;
    this.show();
    this.cueNext();
  }

  /** Gets the hidden video ready on the next clip. */
  private cueNext() {
    const next = this.clips[this.index + 1];
    const back = this.videos[1 - this.front];
    back.pause();
    if (next) cue(back, next, next.start);
  }

  private show() {
    this.videos.forEach((video, i) => video.classList.toggle("front", i === this.front && this.index >= 0));
  }
}

function cue(video: HTMLVideoElement, clip: Clip, time: number) {
  if (video.dataset.path !== clip.path) {
    video.dataset.path = clip.path;
    video.src = mediaUrl(clip.path);
  }
  video.currentTime = Math.min(time + NUDGE, clip.end - NUDGE);
}

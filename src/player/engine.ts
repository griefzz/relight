// Plays a timeline on two video elements: one shows the current clip while the other
// waits on the next, cued up, so a cut to another file doesn't stall.
//
// A recording's clip with its sources set to other levels plays them instead of the
// video's own sound: each source is its own audio element, through a gain (to go past
// 100%), kept in step with the picture.

import { mediaUrl } from "../api";
import { type Clip, clipStarts, locate, totalLength } from "../editor/clips";

/** Just past a frame's start, so float rounding can't show the frame before. */
const NUDGE = 0.001;

/**
 * How early to leave a clip: the end is checked once a screen refresh, so this keeps
 * the frame after it from showing (half a frame at 60 fps).
 */
const EARLY = 0.008;

/** How far a source can drift from the picture before it's put back in step. */
const DRIFT = 0.08;

export interface EngineEvents {
  /** Where playback has got to on the timeline. */
  time: (t: number) => void;
  ended: () => void;
  /** The clip showing can't be played (its path), or can again (null). */
  unplayable: (path: string | null) => void;
}

/** A recording's sound sources as files on their own, once they've been copied out. */
export type SourcesOf = (path: string) => string[] | null;

interface Source {
  audio: HTMLAudioElement;
  gain: GainNode;
  file: string | null;
}

/** A video element and the sources that can play beside it. */
interface Deck {
  video: HTMLVideoElement;
  sources: Source[];
  /** Whether its sound is the sources' rather than the video's. */
  balanced: boolean;
}

export class Engine {
  private decks: [Deck, Deck];
  private front = 0;
  private clips: Clip[] = [];
  private starts: number[] = [];
  private index = -1;
  private playing = false;
  private frame = 0;
  private listeners = new AbortController();
  private context: AudioContext | null = null;

  constructor(
    videos: [HTMLVideoElement, HTMLVideoElement],
    private on: EngineEvents,
    private sourcesOf: SourcesOf,
  ) {
    this.decks = [
      { video: videos[0], sources: [], balanced: false },
      { video: videos[1], sources: [], balanced: false },
    ];
    const signal = this.listeners.signal;
    for (const { video } of this.decks) {
      const showing = () => video === this.decks[this.front].video;
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
    const deck = this.decks[this.front];
    const time = clip.start + at.offset;
    // Mid-play edits re-seek to where it already is; a seek there would only stutter.
    const already =
      this.playing && deck.video.dataset.path === clip.path && Math.abs(deck.video.currentTime - time) < 0.05;
    if (already) this.sound(deck, clip, deck.video.currentTime, false);
    else this.cue(deck, clip, time);
    if (this.playing) this.start(deck);
    this.cueNext();
    this.show();
  }

  /** Picks up sources that weren't ready when the clip showing was cued. */
  refresh() {
    const clip = this.clips[this.index];
    const deck = this.decks[this.front];
    if (!clip) return;
    this.sound(deck, clip, deck.video.currentTime, false);
    if (this.playing) this.start(deck);
  }

  play() {
    if (this.playing || this.index < 0) return;
    this.playing = true;
    this.start(this.decks[this.front]);
    this.frame = requestAnimationFrame(this.tick);
  }

  pause() {
    this.playing = false;
    cancelAnimationFrame(this.frame);
    for (const deck of this.decks) stop(deck);
  }

  destroy() {
    this.pause();
    this.listeners.abort();
    this.context?.close().catch(() => {});
  }

  private tick = () => {
    if (!this.playing) return;
    const clip = this.clips[this.index];
    const deck = this.decks[this.front];
    const video = deck.video;
    if (video.ended || video.currentTime >= clip.end - EARLY) {
      this.advance();
    } else {
      this.on.time(this.starts[this.index] + video.currentTime - clip.start);
      if (deck.balanced) {
        for (const source of deck.sources) {
          if (source.file && Math.abs(source.audio.currentTime - video.currentTime) > DRIFT) {
            source.audio.currentTime = video.currentTime;
          }
        }
      }
    }
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
    const front = this.decks[this.front];
    // The next piece carries straight on in the same file (as after a split): keep going,
    // at its own levels.
    if (next.path === clip.path && Math.abs(next.start - clip.end) < 0.002) {
      this.sound(front, next, front.video.currentTime, false);
      this.start(front);
      this.cueNext();
      return;
    }
    const back = this.decks[1 - this.front];
    if (back.video.dataset.path !== next.path || Math.abs(back.video.currentTime - next.start - NUDGE) > 0.05) {
      this.cue(back, next, next.start);
    }
    this.start(back);
    stop(front);
    this.front = 1 - this.front;
    this.show();
    this.cueNext();
  }

  /** Gets the hidden deck ready on the next clip. */
  private cueNext() {
    const next = this.clips[this.index + 1];
    const back = this.decks[1 - this.front];
    stop(back);
    if (next) this.cue(back, next, next.start);
  }

  private show() {
    this.decks.forEach((deck, i) => deck.video.classList.toggle("front", i === this.front && this.index >= 0));
  }

  private cue(deck: Deck, clip: Clip, time: number) {
    const video = deck.video;
    if (video.dataset.path !== clip.path) {
      video.dataset.path = clip.path;
      video.src = mediaUrl(clip.path);
    }
    const at = Math.min(time + NUDGE, clip.end - NUDGE);
    video.currentTime = at;
    this.sound(deck, clip, at, true);
  }

  /**
   * Gives `deck` the clip's sound: the video's own, or its sources at their levels, put
   * at `time` when `seek` (or when they weren't playing).
   */
  private sound(deck: Deck, clip: Clip, time: number, seek: boolean) {
    const files = this.sourcesOf(clip.path);
    const levels = clip.levels;
    const balanced = !!levels && !!files && files.length === levels.length;
    deck.video.muted = balanced;
    if (!balanced) {
      for (const source of deck.sources) source.audio.pause();
      deck.balanced = false;
      return;
    }
    const context = (this.context ??= new AudioContext());
    while (deck.sources.length < files.length) {
      const audio = new Audio();
      audio.crossOrigin = "anonymous";
      audio.preload = "auto";
      const gain = context.createGain();
      context.createMediaElementSource(audio).connect(gain).connect(context.destination);
      deck.sources.push({ audio, gain, file: null });
    }
    deck.sources.forEach((source, n) => {
      const file = files[n];
      if (!file) {
        source.audio.pause();
        return;
      }
      if (source.file !== file) {
        source.file = file;
        source.audio.src = mediaUrl(file);
        seek = true;
      }
      source.gain.gain.value = levels[n];
      if (seek || !deck.balanced) source.audio.currentTime = time;
    });
    deck.balanced = true;
  }

  /** Plays the deck's picture, and its sources if it's using them. */
  private start(deck: Deck) {
    deck.video.play().catch(() => {});
    if (!deck.balanced) return;
    this.context?.resume().catch(() => {});
    for (const source of deck.sources) if (source.file) source.audio.play().catch(() => {});
  }
}

function stop(deck: Deck) {
  deck.video.pause();
  for (const source of deck.sources) source.audio.pause();
}

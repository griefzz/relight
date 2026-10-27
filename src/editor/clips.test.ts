import { describe, expect, it } from "vitest";
import type { MediaInfo } from "../api";
import { type Clip, clipStarts, cutAt, frameStart, locate, moveTo, totalLength, trimTo } from "./clips";

const clip = (id: string, start: number, end: number, path = "a.mp4"): Clip => ({ id, path, start, end });
const at30 = () => 30;

const media = (duration: number, fps = 30): MediaInfo => ({
  path: "a.mp4",
  duration,
  size: 0,
  video: { codec: "h264", profile: null, pixFmt: null, width: 1920, height: 1080, fps },
  audio: null,
});

describe("locate", () => {
  const clips = [clip("a", 0, 2), clip("b", 10, 13)];

  it("finds the clip and how far into it", () => {
    expect(locate(clips, 0.5)).toEqual({ index: 0, offset: 0.5 });
    expect(locate(clips, 2)).toEqual({ index: 1, offset: 0 });
    expect(locate(clips, 4.5)).toEqual({ index: 1, offset: 2.5 });
  });

  it("past the end is the end of the last clip", () => {
    expect(locate(clips, 99)).toEqual({ index: 1, offset: 3 });
    expect(locate([], 1)).toBeNull();
  });

  it("a clip's start added up a different way still lands in it", () => {
    // The recordings that showed the clip before's last frame.
    const lengths = [6.033333, 7.083333, 2.917];
    const recorded = lengths.map((length, i) => clip(String(i), 0, length));
    const start = totalLength(recorded) - lengths[2];
    expect(start).toBeLessThan(lengths[0] + lengths[1]);
    expect(locate(recorded, start)?.index).toBe(2);
  });
});

describe("cutAt", () => {
  it("cuts at the start of the frame showing", () => {
    const { clips, index } = cutAt([clip("a", 0, 4)], 1.01, at30);
    expect(index).toBe(1);
    expect(clips.map((c) => [c.start, c.end])).toEqual([
      [0, 1],
      [1, 4],
    ]);
    expect(clips[0].id).toBe("a");
    expect(clips[1].id).not.toBe("a");
  });

  it("doesn't cut off less than a frame", () => {
    const one = [clip("a", 0, 4)];
    expect(cutAt(one, 0.001, at30)).toEqual({ clips: one, index: 0 });
    expect(cutAt(one, 3.999, at30).clips).toHaveLength(2);
    expect(cutAt(one, 4, at30)).toEqual({ clips: one, index: 1 });
  });

  it("works across clips", () => {
    const { clips, index } = cutAt([clip("a", 0, 2), clip("b", 5, 8)], 3, at30);
    expect(index).toBe(2);
    expect(clips.map((c) => [c.start, c.end])).toEqual([
      [0, 2],
      [5, 6],
      [6, 8],
    ]);
  });
});

describe("trimTo", () => {
  it("snaps to the nearest frame", () => {
    const [down] = trimTo([clip("a", 0, 4)], "a", "start", 1.01, media(10));
    expect(down.start).toBeCloseTo(1, 9);
    const [up] = trimTo([clip("a", 0, 4)], "a", "start", 1.02, media(10));
    expect(up.start).toBeCloseTo(31 / 30, 9);
  });

  it("keeps a clip at least a frame long, and within its file", () => {
    const [short] = trimTo([clip("a", 0, 4)], "a", "start", 9, media(10));
    expect(short.end - short.start).toBeCloseTo(1 / 30, 9);
    const [long] = trimTo([clip("a", 0, 4)], "a", "end", 50, media(10));
    expect(long.end).toBe(10);
    const [early] = trimTo([clip("a", 2, 4)], "a", "start", -3, media(10));
    expect(early.start).toBe(0);
  });

  it("reaches a file's very end, which needn't be on a frame", () => {
    const [whole] = trimTo([clip("a", 0, 4)], "a", "end", 10.02, media(10.03));
    expect(whole.end).toBe(10.03);
  });
});

describe("moveTo", () => {
  const clips = [clip("a", 0, 1), clip("b", 0, 1), clip("c", 0, 1)];
  const order = (moved: Clip[]) => moved.map((c) => c.id).join("");

  it("moves a clip to before another, or to the end", () => {
    expect(order(moveTo(clips, "a", 2))).toBe("bac");
    expect(order(moveTo(clips, "a", 3))).toBe("bca");
    expect(order(moveTo(clips, "c", 0))).toBe("cab");
  });

  it("leaves things alone when it wouldn't move", () => {
    expect(moveTo(clips, "b", 1)).toBe(clips);
    expect(moveTo(clips, "b", 2)).toBe(clips);
    expect(moveTo(clips, "nope", 0)).toBe(clips);
  });
});

it("frameStart stays on the frame despite float error", () => {
  expect(frameStart(1, 30)).toBe(1);
  expect(frameStart(0.1 * 3, 10)).toBeCloseTo(0.3, 9);
  expect(clipStarts([clip("a", 0, 2), clip("b", 1, 2)])).toEqual([0, 2]);
});

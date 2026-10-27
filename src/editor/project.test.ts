import { describe, expect, it } from "vitest";
import { parse, serialize } from "./project";

describe("project files", () => {
  it("read back what was written, with new ids", () => {
    const clips = [{ id: "x", path: "C:\\a.mp4", start: 1, end: 2 }];
    const read = parse(serialize(["C:\\a.mp4", "C:\\b.mp4"], clips));
    expect(read.media).toEqual(["C:\\a.mp4", "C:\\b.mp4"]);
    expect(read.clips).toHaveLength(1);
    expect(read.clips[0]).toMatchObject({ path: "C:\\a.mp4", start: 1, end: 2 });
    expect(read.clips[0].id).not.toBe("x");
  });

  it("put every clip's file in the bin", () => {
    const text = JSON.stringify({ app: "relight", version: 1, media: [], clips: [{ path: "c.mp4", start: 0, end: 1 }] });
    expect(parse(text).media).toEqual(["c.mp4"]);
  });

  it("keep a clip's levels, within bounds", () => {
    const clips = [{ id: "x", path: "rec.mp4", start: 0, end: 1, levels: [0.5, 1.5] }];
    expect(parse(serialize(["rec.mp4"], clips)).clips[0].levels).toEqual([0.5, 1.5]);
    const text = JSON.stringify({ app: "relight", version: 1, media: [], clips: [{ path: "rec.mp4", start: 0, end: 1, levels: [9, -1] }] });
    expect(parse(text).clips[0].levels).toEqual([2, 0]);
    expect(parse(serialize(["a.mp4"], [{ id: "y", path: "a.mp4", start: 0, end: 1 }])).clips[0]).not.toHaveProperty("levels");
  });

  it("refuse what isn't one", () => {
    expect(() => parse("not json")).toThrow("isn't valid JSON");
    expect(() => parse("{}")).toThrow("isn't a relight project");
    expect(() => parse(JSON.stringify({ app: "relight", version: 2, media: [], clips: [] }))).toThrow("newer relight");
  });
});

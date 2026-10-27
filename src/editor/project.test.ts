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

  it("refuse what isn't one", () => {
    expect(() => parse("not json")).toThrow("isn't valid JSON");
    expect(() => parse("{}")).toThrow("isn't a relight project");
    expect(() => parse(JSON.stringify({ app: "relight", version: 2, media: [], clips: [] }))).toThrow("newer relight");
  });
});

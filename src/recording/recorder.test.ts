import { describe, expect, it } from "vitest";
import { replayDisk, replayMemory } from "./recorder";

describe("how much memory instant replay can use", () => {
  it("covers the length asked for and the bit more it holds", () => {
    expect(replayMemory(30, false, false, false)).toBe(206);
    expect(replayMemory(60, false, false, false)).toBe(394);
  });

  it("adds a little for each sound source", () => {
    expect(replayMemory(30, true, false, false)).toBe(219);
    expect(replayMemory(30, true, true, false)).toBe(226);
  });

  it("bouncing to disk leaves one chunk of picture, however long it holds", () => {
    expect(replayMemory(30, false, false, true)).toBe(13);
    expect(replayMemory(300, false, false, true)).toBe(13);
  });

  it("bouncing to disk leaves the sound", () => {
    expect(replayMemory(300, true, true, true)).toBe(194);
  });
});

describe("how much disk instant replay can use, bouncing", () => {
  it("is what the picture would have taken in memory", () => {
    expect(replayDisk(30)).toBe(replayMemory(30, false, false, false));
    expect(replayDisk(300)).toBe(1894);
  });
});

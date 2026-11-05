import { describe, expect, it } from "vitest";
import { replayMemory } from "./recorder";

describe("how much memory instant replay can use", () => {
  it("covers the length asked for and the bit more it holds", () => {
    expect(replayMemory(30, false, false)).toBe(206);
    expect(replayMemory(60, false, false)).toBe(394);
  });

  it("adds a little for each sound source", () => {
    expect(replayMemory(30, true, false)).toBe(219);
    expect(replayMemory(30, true, true)).toBe(226);
  });
});

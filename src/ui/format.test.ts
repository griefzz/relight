import { expect, it } from "vitest";
import { besidePath, fileName, formatLength, formatMB, formatRuler, formatTime, outputPath } from "./format";

it("times round down, past an hour too", () => {
  expect(formatTime(83.456)).toBe("1:23.45");
  expect(formatTime(6.95)).toBe("0:06.95");
  expect(formatTime(3723.45)).toBe("1:02:03.45");
  expect(formatTime(-1)).toBe("0:00.00");
});

it("lengths round to the nearest hundredth", () => {
  expect(formatLength(1.999)).toBe("0:02.00");
});

it("sizes are decimal megabytes", () => {
  expect(formatMB(9_870_000)).toBe("9.87 MB");
  expect(formatMB(20_000_000)).toBe("20.0 MB");
});

it("paths split on either slash", () => {
  expect(fileName("C:\\clips\\run.mov")).toBe("run.mov");
  expect(fileName("/home/clips/run.mov")).toBe("run.mov");
  expect(outputPath("C:\\clips\\run.mov", "_discord")).toBe("C:\\clips\\run_discord.mp4");
  expect(outputPath("C:\\cl.ips\\run", "_full")).toBe("C:\\cl.ips\\run_full.mp4");
  expect(besidePath("C:\\clips\\run.mov", "Untitled.relight")).toBe("C:\\clips\\Untitled.relight");
});

it("ruler labels are as precise as their spacing", () => {
  expect(formatRuler(90, 30)).toBe("1:30");
  expect(formatRuler(1.5, 0.5)).toBe("0:01.5");
});

// Checks the ffmpeg in src-tauri/ffmpeg has everything relight uses, so a newer build that
// drops something fails the build rather than someone's export or recording.

import { execFileSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const dir = join(dirname(fileURLToPath(import.meta.url)), "..", "src-tauri", "ffmpeg");

const needed = {
  // Exporting (x264, SVT-AV1, AAC), recording (NVENC), and the in-between files.
  encoders: ["libx264", "libsvtav1", "h264_nvenc", "aac", "pcm_s16le", "pcm_f32le", "mjpeg"],
  filters: [
    "ddagrab", "hwdownload", "fps", "scale", "pad", "tpad", "trim", "setpts", "setsar", "format", "concat", "tile",
    "aresample", "aformat", "apad", "atrim", "asetpts", "anullsrc", "pan", "asplit", "amix", "anull",
  ],
  formats: ["mp4", "mov", "matroska", "concat", "image2", "f32le", "s16le", "lavfi", "null"],
};

/** The names `ffmpeg -<kind>` lists, split where one line lists several ("mov,mp4,m4a"). */
function names(kind) {
  const out = execFileSync(join(dir, "ffmpeg.exe"), ["-hide_banner", `-${kind}`], { encoding: "utf8" });
  // Formats have flag columns that can be blank (" D d lavfi", "  E  mp4"); the others'
  // flags are one word (" V....D libx264").
  const name = (line) =>
    kind === "formats" ? (line.match(/^\s[D ][E ][d ]?\s+(\S+)/)?.[1] ?? "") : (line.trim().split(/\s+/)[1] ?? "");
  return new Set(out.split("\n").flatMap((line) => name(line).split(",")));
}

const missing = Object.entries(needed).flatMap(([kind, wanted]) => {
  const have = names(kind);
  return wanted.filter((name) => !have.has(name)).map((name) => `${kind}: ${name}`);
});
execFileSync(join(dir, "ffprobe.exe"), ["-version"]);

if (missing.length) {
  console.error(`The bundled ffmpeg is missing what relight needs:\n  ${missing.join("\n  ")}`);
  process.exit(1);
}
console.log("The bundled ffmpeg has everything relight uses.");

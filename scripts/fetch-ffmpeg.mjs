// Puts the ffmpeg the installers ship with in src-tauri/ffmpeg: a GPL build from
// BtbN/FFmpeg-Builds (x264 and SVT-AV1 for exporting, NVENC and ddagrab for recording),
// checked against its known hash. `npm run installers` runs this first.
//
// To move to a newer ffmpeg, pick a month-end autobuild (those are kept for about two
// years; the daily ones only for two weeks) and update the three lines below.

import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const release = "autobuild-2026-09-30-13-08";
const name = "ffmpeg-n9.0.2-17-g2a571b6068-win64-gpl-shared-9.0";
const sha256 = "3da6c7b60bb9ccd73ec5b5e815ba804a0879eb362ba0e3beebce50174c022696";

const dest = join(dirname(fileURLToPath(import.meta.url)), "..", "src-tauri", "ffmpeg");
const marker = join(dest, "VERSION");
if (existsSync(marker) && readFileSync(marker, "utf8").trim() === name) {
  console.log(`ffmpeg is already there: ${name}`);
  process.exit(0);
}

const url = `https://github.com/BtbN/FFmpeg-Builds/releases/download/${release}/${name}.zip`;
console.log(`Downloading ${name}...`);
const response = await fetch(url);
if (!response.ok) throw new Error(`Couldn't download ${url}: ${response.status} ${response.statusText}`);
const zip = Buffer.from(await response.arrayBuffer());
const got = createHash("sha256").update(zip).digest("hex");
if (got !== sha256) throw new Error(`The download's SHA-256 is ${got}, not ${sha256}: not using it.`);

const work = mkdtempSync(join(tmpdir(), "relight-ffmpeg-"));
try {
  const archive = join(work, `${name}.zip`);
  writeFileSync(archive, zip);
  // Windows' own tar reads zips; Git's, which can come first on the PATH, doesn't.
  execFileSync(join(process.env.SystemRoot ?? "C:\\Windows", "System32", "tar.exe"), ["-xf", archive, "-C", work]);

  // Only what relight runs: ffmpeg, ffprobe and the libraries they share.
  rmSync(dest, { recursive: true, force: true });
  mkdirSync(dest, { recursive: true });
  const bin = join(work, name, "bin");
  for (const file of readdirSync(bin)) {
    if (file === "ffmpeg.exe" || file === "ffprobe.exe" || file.endsWith(".dll")) cpSync(join(bin, file), join(dest, file));
  }
  cpSync(join(work, name, "LICENSE.txt"), join(dest, "LICENSE.txt"));
  const version = name.replace(/^ffmpeg-(n[\d.]+-\d+-g[0-9a-f]+)-.*$/, "$1");
  const commit = version.replace(/^.*-g/, "");
  writeFileSync(
    join(dest, "README.txt"),
    `This is FFmpeg ${version}, as built by BtbN/FFmpeg-Builds (${release}),
under the GNU GPL version 3: see LICENSE.txt.

FFmpeg's source:   https://github.com/FFmpeg/FFmpeg/tree/${commit}
How it was built:  https://github.com/BtbN/FFmpeg-Builds
The build itself:  https://github.com/BtbN/FFmpeg-Builds/releases/tag/${release}
`,
  );
  writeFileSync(marker, name);

  const bytes = readdirSync(dest).reduce((sum, file) => sum + statSync(join(dest, file)).size, 0);
  console.log(`ffmpeg is in src-tauri/ffmpeg (${(bytes / 1024 / 1024).toFixed(1)} MB)`);
} finally {
  rmSync(work, { recursive: true, force: true });
}

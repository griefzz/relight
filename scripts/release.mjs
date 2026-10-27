// The release workflow's steps around the build, for a version tag ("v0.2.0"):
//
//   node scripts/release.mjs check v0.2.0     the tag is the version in Cargo.toml
//   node scripts/release.mjs publish v0.2.0   a GitHub release of the signed installer,
//                                             with the latest.json relight updates from

import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const [step, tag] = process.argv.slice(2);

const cargo = readFileSync(join(root, "Cargo.toml"), "utf8");
const version = cargo.match(/\[workspace\.package\][^[]*?\bversion\s*=\s*"([^"]+)"/)?.[1];
if (!version) throw new Error("Couldn't find the version in Cargo.toml's [workspace.package].");
if (tag !== `v${version}`) {
  console.error(`The tag is ${tag}, but Cargo.toml says ${version}: tag v${version}, or change the version.`);
  process.exit(1);
}

if (step === "check") {
  console.log(`Releasing ${version}.`);
} else if (step === "publish") {
  const repo = process.env.GITHUB_REPOSITORY;
  if (!repo) throw new Error("GITHUB_REPOSITORY isn't set: this runs in GitHub Actions.");
  const name = `relight_${version}_x64-setup.exe`;
  const installer = join(root, "target", "release", "bundle", "nsis", name);
  const signature = `${installer}.sig`;
  for (const file of [installer, signature]) {
    if (!existsSync(file)) throw new Error(`${file} wasn't built.`);
  }
  // What relight reads to see there's a newer version, and where to get it.
  const latest = join(root, "target", "latest.json");
  writeFileSync(
    latest,
    JSON.stringify(
      {
        version,
        notes: `relight ${version}`,
        pub_date: new Date().toISOString(),
        platforms: {
          "windows-x86_64": {
            signature: readFileSync(signature, "utf8").trim(),
            url: `https://github.com/${repo}/releases/download/${tag}/${name}`,
          },
        },
      },
      null,
      2,
    ),
  );
  execFileSync("gh", ["release", "create", tag, installer, signature, latest, "--title", `relight ${version}`, "--generate-notes"], {
    stdio: "inherit",
  });
} else {
  console.error("usage: node scripts/release.mjs check|publish <tag>");
  process.exit(2);
}

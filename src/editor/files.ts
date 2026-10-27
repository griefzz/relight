// Opening and saving: media, projects, and the dialogs for them.

import { ask, open, save } from "@tauri-apps/plugin-dialog";
import { type MediaInfo, loadProjectFile, openMedia, saveProjectFile } from "../api";
import { besidePath, fileName } from "../ui/format";
import { parse, serialize } from "./project";
import { addMedia, appendMedia, loadProject, markSaved, newProject, useEditor } from "./store";

export const VIDEO_EXTENSIONS = ["mp4", "mov", "mkv", "webm", "m4v", "avi", "ts", "flv"];

const PROJECT_FILTER = { name: "relight project", extensions: ["relight"] };

const isProject = (path: string) => /\.relight$/i.test(path);

/** Asks before throwing away unsaved changes; true to go ahead. */
export async function okToDiscard(): Promise<boolean> {
  if (!useEditor.getState().dirty) return true;
  return ask("Your changes to this project haven't been saved. Discard them?", {
    title: "Unsaved changes",
    kind: "warning",
    okLabel: "Discard",
    cancelLabel: "Keep editing",
  });
}

/** Adds videos to the bin and the end of the timeline. A project among them opens instead. */
export async function openPaths(paths: string[]) {
  const project = paths.find(isProject);
  if (project) return openProject(project);

  const errors: string[] = [];
  for (const path of paths) {
    try {
      addMedia(await openMedia(path));
      appendMedia(path);
    } catch (e) {
      errors.push(String(e));
    }
  }
  if (errors.length) throw new Error(errors.join("\n"));
}

export async function importMedia() {
  const picked = await open({ multiple: true, filters: [{ name: "Video", extensions: VIDEO_EXTENSIONS }] });
  if (picked) await openPaths(picked);
}

export async function chooseProject() {
  const path = await open({ filters: [PROJECT_FILTER] });
  if (path) await openProject(path);
}

export async function openProject(path: string) {
  if (!(await okToDiscard())) return;
  const { media, clips } = parse(await loadProjectFile(path));
  const found = new Map<string, MediaInfo>();
  const tries = await Promise.allSettled(media.map(openMedia));
  tries.forEach((result, i) => result.status === "fulfilled" && found.set(media[i], result.value));
  const missing = media.filter((file) => !found.has(file));

  // Where missing files turned up, by where the project had them.
  const moved = new Map<string, string>();
  if (missing.length) {
    const list = missing.map((file) => `  ${fileName(file)}`).join("\n");
    const look = await ask(`${fileName(path)} uses files that aren't where they were:\n\n${list}\n\nFind them?`, {
      title: "Missing media",
      kind: "warning",
      okLabel: "Find…",
      cancelLabel: "Don't open",
    });
    if (!look) return;
    for (const lost of missing) {
      if (moved.has(lost)) continue;
      const extension = lost.match(/\.([^.\\/]+)$/)?.[1];
      const there = await open({
        title: `Where is ${fileName(lost)}?`,
        defaultPath: besidePath(path, ""),
        filters: [
          ...(extension ? [{ name: fileName(lost), extensions: [extension] }] : []),
          { name: "Video", extensions: VIDEO_EXTENSIONS },
        ],
      });
      if (!there) return;
      found.set(there, await openMedia(there));
      moved.set(lost, there);
      // Files usually move together, so the rest may be in the same folder.
      for (const other of missing) {
        if (moved.has(other)) continue;
        const beside = besidePath(there, fileName(other));
        try {
          found.set(beside, await openMedia(beside));
          moved.set(other, beside);
        } catch {
          // Not there; it gets asked for in turn.
        }
      }
    }
  }

  const now = (file: string) => moved.get(file) ?? file;
  // A file found elsewhere may be a different length: clips keep within it.
  const relinked = clips
    .map((clip) => {
      const duration = found.get(now(clip.path))!.duration;
      return { ...clip, path: now(clip.path), start: Math.min(clip.start, duration), end: Math.min(clip.end, duration) };
    })
    .filter((clip) => clip.end > clip.start);
  const bin = [...new Set(media.map(now))].map((file) => found.get(file)!);
  loadProject(bin, relinked, path, moved.size > 0);
}

/** Saves the project, asking where when it's new or when `as` is set. */
export async function saveProject(as = false) {
  const { media, clips, projectPath } = useEditor.getState();
  let path = projectPath;
  if (as || !path) {
    const nearby = clips[0]?.path ?? Object.keys(media)[0];
    path = await save({
      defaultPath: nearby ? besidePath(nearby, "Untitled.relight") : "Untitled.relight",
      filters: [PROJECT_FILTER],
    });
    if (!path) return;
  }
  await saveProjectFile(path, serialize(Object.keys(media), clips));
  markSaved(path);
}

export async function startNewProject() {
  if (await okToDiscard()) newProject();
}

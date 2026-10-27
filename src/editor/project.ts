// The .relight project file: the media bin and the timeline, as JSON.

import { type Clip, newId } from "./clips";

interface ProjectFile {
  app: "relight";
  version: 1;
  /** The media bin, in order. */
  media: string[];
  clips: { path: string; start: number; end: number }[];
}

export function serialize(media: string[], clips: Clip[]): string {
  const file: ProjectFile = {
    app: "relight",
    version: 1,
    media,
    clips: clips.map(({ path, start, end }) => ({ path, start, end })),
  };
  return JSON.stringify(file, null, 2);
}

export function parse(text: string): { media: string[]; clips: Clip[] } {
  let file: ProjectFile;
  try {
    file = JSON.parse(text);
  } catch {
    throw new Error("That isn't a relight project: it isn't valid JSON.");
  }
  if (file?.app !== "relight" || !Array.isArray(file.media) || !Array.isArray(file.clips)) {
    throw new Error("That isn't a relight project.");
  }
  if (file.version > 1) throw new Error("That project was saved by a newer relight.");

  const clips = file.clips.map((clip) => ({
    id: newId(),
    path: String(clip.path),
    start: Number(clip.start),
    end: Number(clip.end),
  }));
  // Every clip's media belongs in the bin, even if the file's list missed one.
  const media = [...new Set([...file.media.map(String), ...clips.map((clip) => clip.path)])];
  return { media, clips };
}

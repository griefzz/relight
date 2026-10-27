// Each media file gets a colour, so its clips can be told apart on the timeline.

const COLORS = ["#3f5f86", "#5f6f3b", "#7a4f6d", "#3f6f6b", "#7f5c39", "#55507f"];

export function colorFor(path: string): string {
  let hash = 0;
  for (let i = 0; i < path.length; i++) hash = (hash * 31 + path.charCodeAt(i)) | 0;
  return COLORS[Math.abs(hash) % COLORS.length];
}

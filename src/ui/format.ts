/**
 * A point in time: 83.456 → "1:23.45", and "1:02:03.45" past an hour. Rounds down, so
 * it never reads a moment it hasn't reached.
 */
export function formatTime(seconds: number): string {
  // The nudge keeps float error (6.95 * 100 = 694.99…) from losing a hundredth.
  const hundredths = Math.floor(Math.max(0, seconds) * 100 + 1e-6);
  const h = Math.floor(hundredths / 360_000);
  const m = Math.floor(hundredths / 6000) % 60;
  const s = ((hundredths % 6000) / 100).toFixed(2).padStart(5, "0");
  return h ? `${h}:${String(m).padStart(2, "0")}:${s}` : `${m}:${s}`;
}

/** A length of time, to the nearest hundredth, so lengths that add up read that way. */
export const formatLength = (seconds: number) => formatTime(Math.round(seconds * 100) / 100);

/** Decimal megabytes, the way Discord's limit counts them. */
export function formatMB(bytes: number): string {
  const mb = bytes / 1000 / 1000;
  return `${mb.toFixed(mb < 10 ? 2 : 1)} MB`;
}

export function fileName(path: string): string {
  return path.split(/[\\/]/).pop() ?? path;
}

/** "C:\clips\run.mov" + "_discord" → "C:\clips\run_discord.mp4" */
export function outputPath(input: string, suffix: string): string {
  return `${input.replace(/\.[^.\\/]*$/, "")}${suffix}.mp4`;
}

/** "C:\clips\run.mov" + "Untitled.relight" → "C:\clips\Untitled.relight" */
export function besidePath(path: string, name: string): string {
  return path.replace(/[^\\/]*$/, name);
}

/** Ruler labels: as precise as the spacing between them needs. */
export function formatRuler(seconds: number, step: number): string {
  const whole = Math.round(seconds * 100) / 100;
  const h = Math.floor(whole / 3600);
  const m = Math.floor(whole / 60) % 60;
  const s = whole % 60;
  const secs = step < 1 ? s.toFixed(1).padStart(4, "0") : String(Math.floor(s)).padStart(2, "0");
  return h ? `${h}:${String(m).padStart(2, "0")}:${secs}` : `${m}:${secs}`;
}

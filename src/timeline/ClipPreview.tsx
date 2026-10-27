import { useEffect, useRef } from "react";
import { type Clip, clamp, clipLength } from "../editor/clips";
import { type Preview, usePreviews } from "./previews";

/** Heights of a clip's thumbnails and waveform, matching the stylesheet. */
const FILM_HEIGHT = 48;
const WAVE_HEIGHT = 24;

/**
 * A clip's thumbnails and waveform, for the part of it in view: zoomed in on a long clip,
 * the whole of it would be far wider than anything a browser draws.
 */
export function ClipPreview(props: { clip: Clip; left: number; zoom: number; view: { left: number; width: number } }) {
  const { clip, left, zoom, view } = props;
  const preview = usePreviews((s) => s[clip.path]);
  const from = Math.max(0, view.left - left);
  const to = Math.min(clipLength(clip) * zoom, view.left + view.width - left);
  if (!preview || to <= from) return null;
  const span = { clip, zoom, from, to };
  return (
    <>
      {preview.stripUrl && <Filmstrip preview={preview} {...span} />}
      {preview.peaks && <Waveform peaks={preview.peaks} perSecond={preview.peaksPerSecond} {...span} />}
    </>
  );
}

interface Span {
  clip: Clip;
  zoom: number;
  /** The part to draw, in pixels from the clip's left edge. */
  from: number;
  to: number;
}

function Filmstrip({ preview, clip, zoom, from, to }: Span & { preview: Preview }) {
  const width = (preview.tileWidth * FILM_HEIGHT) / preview.tileHeight;
  const tiles = [];
  for (let slot = Math.floor(from / width); slot * width < to; slot++) {
    // The thumbnail for the middle of this slot.
    const t = clip.start + ((slot + 0.5) * width) / zoom;
    const tile = clamp(Math.floor(t / preview.step), 0, preview.tiles - 1);
    tiles.push(
      <div
        key={slot}
        className="film-tile"
        style={{
          left: slot * width,
          width,
          backgroundImage: `url("${preview.stripUrl}")`,
          backgroundSize: `${preview.tiles * width}px ${FILM_HEIGHT}px`,
          backgroundPosition: `${-tile * width}px 0`,
        }}
      />,
    );
  }
  return <div className="filmstrip">{tiles}</div>;
}

function Waveform({ peaks, perSecond, clip, zoom, from, to }: Span & { peaks: Uint8Array; perSecond: number }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const width = Math.ceil(to - from);
  useEffect(() => {
    const c = canvas.current;
    if (!c) return;
    const scale = window.devicePixelRatio || 1;
    c.width = width * scale;
    c.height = WAVE_HEIGHT * scale;
    const g = c.getContext("2d")!;
    g.scale(scale, scale);
    g.fillStyle = "rgb(255 255 255 / 0.72)";
    for (let x = 0; x < width; x++) {
      // The loudest peak under this column of pixels.
      const first = Math.floor((clip.start + (from + x) / zoom) * perSecond);
      const last = Math.max(first + 1, Math.ceil((clip.start + (from + x + 1) / zoom) * perSecond));
      let loudest = 0;
      for (let i = first; i < last && i < peaks.length; i++) loudest = Math.max(loudest, peaks[i]);
      // Square root, so quiet sound still shows.
      const h = Math.max(1, Math.sqrt(loudest / 255) * WAVE_HEIGHT);
      g.fillRect(x, (WAVE_HEIGHT - h) / 2, 1, h);
    }
  }, [peaks, perSecond, clip.start, zoom, from, width]);
  return <canvas ref={canvas} className="waveform" style={{ left: from, width, height: WAVE_HEIGHT }} />;
}

import { useEffect, useLayoutEffect, useMemo, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import { fileName, formatLength, formatRuler, formatTime } from "../ui/format";
import { colorFor } from "../ui/palette";
import { follow, setDropTarget } from "./pointer";
import { ClipPreview } from "./ClipPreview";
import {
  beginDrag,
  clearSelection,
  deleteSelected,
  endDrag,
  moveClip,
  redo,
  seek,
  select,
  split,
  trimClip,
  undo,
  useEditor,
} from "../editor/store";
import { type Clip, type Edge, clamp, clipLength, clipStarts, totalLength } from "../editor/clips";

/** Pixels per second. */
const MIN_ZOOM = 1;
const MAX_ZOOM = 800;
const TICK_STEPS = [0.1, 0.2, 0.5, 1, 2, 5, 10, 15, 30, 60, 120, 300, 600, 900, 1800, 3600];
const TICK_GAP = 80;
/** Empty timeline after the last clip, to drop a moved clip into. */
const TAIL = 32;

const toSlider = (zoom: number) => (Math.log(zoom / MIN_ZOOM) / Math.log(MAX_ZOOM / MIN_ZOOM)) * 1000;
const fromSlider = (value: number) => MIN_ZOOM * (MAX_ZOOM / MIN_ZOOM) ** (value / 1000);

/** The clip being dragged to a new place: how far it's moved, and where it would go. */
interface Moving {
  id: string;
  dx: number;
  to: number;
}

export function Timeline() {
  const clips = useEditor((s) => s.clips);
  const selection = useEditor((s) => s.selection);
  const canUndo = useEditor((s) => s.past.length > 0);
  const canRedo = useEditor((s) => s.future.length > 0);
  const scroller = useRef<HTMLDivElement>(null);
  const content = useRef<HTMLDivElement>(null);
  const [zoom, setZoom] = useState(40);
  const [view, setView] = useState({ left: 0, width: 800 });
  const [moving, setMoving] = useState<Moving | null>(null);
  /** Where media dragged from the bin would go. */
  const [incoming, setIncoming] = useState<number | null>(null);
  const scrollAfterZoom = useRef<number | null>(null);

  const starts = useMemo(() => clipStarts(clips), [clips]);
  const total = totalLength(clips);
  const boundaries = [...starts, total];

  /** Zooms keeping the time under `anchor` (pixels from the left of the view) where it is. */
  const zoomTo = (next: number, anchor = view.width / 2) => {
    const el = scroller.current;
    if (!el) return;
    next = clamp(next, MIN_ZOOM, MAX_ZOOM);
    scrollAfterZoom.current = Math.max(0, ((el.scrollLeft + anchor) / zoom) * next - anchor);
    setZoom(next);
  };
  const latest = useRef({ zoom, zoomTo });
  latest.current = { zoom, zoomTo };

  const fit = () => {
    const el = scroller.current;
    if (!total || !el) return;
    scrollAfterZoom.current = 0;
    setZoom(clamp((el.clientWidth - TAIL - 8) / total, MIN_ZOOM, MAX_ZOOM));
  };

  useLayoutEffect(() => {
    if (scrollAfterZoom.current !== null && scroller.current) {
      scroller.current.scrollLeft = scrollAfterZoom.current;
      scrollAfterZoom.current = null;
    }
  }, [zoom]);

  // The whole timeline in view when the first clips arrive.
  const hadClips = useRef(false);
  useEffect(() => {
    if (clips.length && !hadClips.current) fit();
    hadClips.current = clips.length > 0;
  }, [clips.length]);

  useEffect(() => {
    const el = scroller.current!;
    const measure = () => setView({ left: el.scrollLeft, width: el.clientWidth });
    measure();
    const resized = new ResizeObserver(measure);
    resized.observe(el);
    el.addEventListener("scroll", measure, { passive: true });
    // Ctrl+wheel zooms around the pointer; the wheel alone scrolls along.
    const wheel = (e: WheelEvent) => {
      e.preventDefault();
      if (e.ctrlKey) {
        const { zoom, zoomTo } = latest.current;
        zoomTo(zoom * Math.exp(-e.deltaY * 0.002), e.clientX - el.getBoundingClientRect().left);
      } else {
        el.scrollLeft += e.deltaY + e.deltaX;
      }
    };
    el.addEventListener("wheel", wheel, { passive: false });
    return () => {
      resized.disconnect();
      el.removeEventListener("scroll", measure);
      el.removeEventListener("wheel", wheel);
    };
  }, []);

  // Keeps the playhead in view while playing.
  useEffect(
    () =>
      useEditor.subscribe((s, before) => {
        const el = scroller.current;
        if (!el || !s.playing || s.playhead === before.playhead) return;
        const x = s.playhead * latest.current.zoom;
        if (x < el.scrollLeft || x > el.scrollLeft + el.clientWidth - 24) el.scrollLeft = x - 24;
      }),
    [],
  );

  const timeAt = (clientX: number) => (clientX - content.current!.getBoundingClientRect().left) / zoom;

  const nearestBoundary = (t: number) =>
    boundaries.reduce((best, b, i) => (Math.abs(b - t) < Math.abs(boundaries[best] - t) ? i : best), 0);

  const forDrop = useRef({ timeAt, nearestBoundary });
  forDrop.current = { timeAt, nearestBoundary };
  useEffect(() => {
    setDropTarget({
      indexAt: (x, y) => {
        const box = scroller.current?.getBoundingClientRect();
        if (!box || x < box.left || x > box.right || y < box.top || y > box.bottom) return null;
        const { timeAt, nearestBoundary } = forDrop.current;
        return nearestBoundary(timeAt(x));
      },
      show: setIncoming,
    });
    return () => setDropTarget(null);
  }, []);

  const scrub = (e: ReactPointerEvent<HTMLElement>) => {
    if (e.button !== 0) return;
    seek(timeAt(e.clientX));
    follow(e, (ev) => seek(timeAt(ev.clientX)));
  };

  const pressClip = (e: ReactPointerEvent<HTMLElement>, clip: Clip) => {
    if (e.button !== 0) return;
    e.stopPropagation();
    if (e.ctrlKey || e.shiftKey || e.metaKey) {
      select(clip.id, true);
      return;
    }
    if (!selection.includes(clip.id)) select(clip.id);
    const x0 = e.clientX;
    let dragged = false;
    follow(
      e,
      (ev) => {
        const dx = ev.clientX - x0;
        if (!dragged && Math.abs(dx) < 4) return;
        dragged = true;
        setMoving({ id: clip.id, dx, to: nearestBoundary(timeAt(ev.clientX)) });
      },
      (ev) => {
        if (dragged) moveClip(clip.id, nearestBoundary(timeAt(ev.clientX)));
        else select(clip.id);
        setMoving(null);
      },
    );
  };

  const pressEdge = (e: ReactPointerEvent<HTMLElement>, clip: Clip, edge: Edge) => {
    if (e.button !== 0) return;
    e.stopPropagation();
    select(clip.id);
    beginDrag();
    const x0 = e.clientX;
    const from = edge === "start" ? clip.start : clip.end;
    follow(e, (ev) => trimClip(clip.id, edge, from + (ev.clientX - x0) / zoom), endDrag);
  };

  return (
    <section className="timeline">
      <div className="timeline-tools">
        <button onClick={split} disabled={!clips.length} title="Split the clip at the playhead (S)">
          Split
        </button>
        <button onClick={deleteSelected} disabled={!selection.length} title="Delete the selected clips (Delete)">
          Delete
        </button>
        <span className="divider" />
        <button onClick={undo} disabled={!canUndo} title="Undo (Ctrl+Z)">
          Undo
        </button>
        <button onClick={redo} disabled={!canRedo} title="Redo (Ctrl+Shift+Z)">
          Redo
        </button>
        <div className="spacer" />
        <input
          className="zoom"
          type="range"
          min={0}
          max={1000}
          value={toSlider(zoom)}
          onChange={(e) => zoomTo(fromSlider(Number(e.target.value)))}
          onPointerUp={(e) => e.currentTarget.blur()}
          aria-label="Zoom"
          title="Zoom (Ctrl+wheel over the timeline)"
        />
        <button onClick={fit} disabled={!clips.length} title="Fit the whole timeline in view">
          Fit
        </button>
      </div>

      <div className="timeline-scroll" ref={scroller}>
        <div className="timeline-content" ref={content} style={{ width: Math.max(view.width, total * zoom + TAIL) }}>
          <Ruler zoom={zoom} view={view} onPointerDown={scrub} />
          <div
            className="track"
            onPointerDown={(e) => {
              clearSelection();
              scrub(e);
            }}
          >
            {clips.map((clip, i) => {
              const isMoving = moving?.id === clip.id;
              const name = fileName(clip.path);
              return (
                <div
                  key={clip.id}
                  className={`clip${selection.includes(clip.id) ? " selected" : ""}${isMoving ? " moving" : ""}`}
                  style={{
                    left: starts[i] * zoom + (isMoving ? moving.dx : 0),
                    width: Math.max(clipLength(clip) * zoom, 3),
                    background: colorFor(clip.path),
                  }}
                  title={`${name}\n${formatTime(clip.start)} – ${formatTime(clip.end)} of the file`}
                  onPointerDown={(e) => pressClip(e, clip)}
                >
                  <div
                    className="clip-label"
                    // Kept in view when the clip starts off to the left.
                    style={{ paddingLeft: 9 + clamp(view.left - starts[i] * zoom, 0, clipLength(clip) * zoom - 120) }}
                  >
                    <span className="clip-name">{name}</span>
                    <span className="clip-length">{formatLength(clipLength(clip))}</span>
                  </div>
                  <ClipPreview clip={clip} left={starts[i] * zoom} zoom={zoom} view={view} />
                  <div className="clip-edge start" onPointerDown={(e) => pressEdge(e, clip, "start")} />
                  <div className="clip-edge end" onPointerDown={(e) => pressEdge(e, clip, "end")} />
                </div>
              );
            })}
            {moving && <div className="drop-marker" style={{ left: boundaries[moving.to] * zoom }} />}
            {incoming !== null && <div className="drop-marker" style={{ left: boundaries[incoming] * zoom }} />}
            {!clips.length && <div className="track-empty">Imported videos go here, end to end.</div>}
          </div>
          <Playhead zoom={zoom} />
        </div>
      </div>
    </section>
  );
}

function Ruler(props: {
  zoom: number;
  view: { left: number; width: number };
  onPointerDown: (e: ReactPointerEvent<HTMLElement>) => void;
}) {
  const { zoom, view } = props;
  const step = TICK_STEPS.find((s) => s * zoom >= TICK_GAP) ?? TICK_STEPS[TICK_STEPS.length - 1];
  const ticks = [];
  // Only the ticks in view, so a long timeline zoomed in stays quick.
  for (let k = Math.floor(view.left / zoom / step); k <= Math.ceil((view.left + view.width) / zoom / step); k++) {
    const t = k * step;
    ticks.push(
      <div key={k} className="tick major" style={{ left: t * zoom }}>
        <span>{formatRuler(t, step)}</span>
      </div>,
    );
    for (let j = 1; j < 5; j++) {
      ticks.push(<div key={`${k}.${j}`} className="tick" style={{ left: (t + (j * step) / 5) * zoom }} />);
    }
  }
  return (
    <div className="ruler" onPointerDown={props.onPointerDown}>
      {ticks}
    </div>
  );
}

function Playhead({ zoom }: { zoom: number }) {
  const playhead = useEditor((s) => s.playhead);
  return <div className="playhead" style={{ transform: `translateX(${playhead * zoom}px)` }} />;
}

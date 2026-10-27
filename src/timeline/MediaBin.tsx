import { useState, type PointerEvent as ReactPointerEvent } from "react";
import { importMedia } from "../editor/files";
import { fileName, formatLength } from "../ui/format";
import { colorFor } from "../ui/palette";
import { dropTarget, follow } from "./pointer";
import { insertMedia, insertMediaAt, useEditor } from "../editor/store";

/** A video being dragged out of the bin, and where the pointer is. */
interface Carrying {
  path: string;
  x: number;
  y: number;
}

export function MediaBin({ onError }: { onError: (error: unknown) => void }) {
  const media = useEditor((s) => s.media);
  const items = Object.values(media);
  const [carrying, setCarrying] = useState<Carrying | null>(null);

  // Dragged onto the timeline, a video goes in between the clips nearest the pointer.
  const press = (e: ReactPointerEvent<HTMLLIElement>, path: string) => {
    if (e.button !== 0 || (e.target instanceof Element && e.target.closest("button"))) return;
    const x0 = e.clientX;
    const y0 = e.clientY;
    let dragged = false;
    follow(
      e,
      (ev) => {
        if (!dragged && Math.hypot(ev.clientX - x0, ev.clientY - y0) < 5) return;
        dragged = true;
        setCarrying({ path, x: ev.clientX, y: ev.clientY });
        dropTarget()?.show(dropTarget()?.indexAt(ev.clientX, ev.clientY) ?? null);
      },
      (ev) => {
        setCarrying(null);
        const target = dropTarget();
        target?.show(null);
        const index = dragged ? target?.indexAt(ev.clientX, ev.clientY) : null;
        if (index != null) insertMediaAt(path, index);
      },
    );
  };

  return (
    <aside className="bin">
      <header>
        <span>Media</span>
        <button onClick={() => importMedia().catch(onError)} title="Import videos (Ctrl+I)">
          Import…
        </button>
      </header>
      {items.length ? (
        <ul>
          {items.map((info) => (
            <li
              key={info.path}
              title={`${info.path}\nDrag onto the timeline, or double-click to insert at the playhead`}
              onPointerDown={(e) => press(e, info.path)}
              onDoubleClick={() => insertMedia(info.path)}
            >
              <span className="swatch" style={{ background: colorFor(info.path) }} />
              <span className="bin-text">
                <span className="bin-name">{fileName(info.path)}</span>
                <span className="bin-meta">
                  {info.video ? `${info.video.width}×${info.video.height} · ` : ""}
                  {formatLength(info.duration)}
                </span>
              </span>
              <button className="icon small" title="Insert at the playhead" onClick={() => insertMedia(info.path)}>
                +
              </button>
            </li>
          ))}
        </ul>
      ) : (
        <p className="bin-empty">Imported videos are listed here.</p>
      )}
      {carrying && (
        <div className="carrying" style={{ left: carrying.x, top: carrying.y }}>
          <span className="swatch" style={{ background: colorFor(carrying.path) }} />
          {fileName(carrying.path)}
        </div>
      )}
    </aside>
  );
}

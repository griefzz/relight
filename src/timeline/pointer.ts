// Following a pointer drag, and handing media dragged from the bin to the timeline. The
// window's own drag and drop belongs to Tauri, for files from outside, so drags inside
// the window are followed by hand.

import type { PointerEvent as ReactPointerEvent } from "react";

/** Calls `move` as the pointer that pressed moves, and `up` when it lets go. */
export function follow(
  e: ReactPointerEvent<HTMLElement>,
  move: (ev: PointerEvent) => void,
  up?: (ev: PointerEvent) => void,
) {
  const el = e.currentTarget;
  el.setPointerCapture(e.pointerId);
  const done = (ev: PointerEvent) => {
    el.removeEventListener("pointermove", move);
    el.removeEventListener("pointerup", done);
    el.removeEventListener("pointercancel", done);
    up?.(ev);
  };
  el.addEventListener("pointermove", move);
  el.addEventListener("pointerup", done);
  el.addEventListener("pointercancel", done);
}

/** Somewhere media can be dropped: the timeline. */
export interface DropTarget {
  /** Where the media would go if let go at (`x`, `y`), or null when that's not over it. */
  indexAt(x: number, y: number): number | null;
  /** Shows where it would go, or with null, stops. */
  show(index: number | null): void;
}

let target: DropTarget | null = null;

export const setDropTarget = (next: DropTarget | null) => {
  target = next;
};

export const dropTarget = () => target;

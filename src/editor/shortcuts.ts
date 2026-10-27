// The editor's keys, all in one place (the README's table lists them).

import { useEffect, useRef } from "react";
import { totalLength } from "./clips";
import { setFullscreen, toggleFullscreen, useFullscreen } from "../player/fullscreen";
import { chooseProject, importMedia, saveProject, startNewProject } from "./files";
import {
  clearSelection,
  copySelection,
  cutSelection,
  deleteSelected,
  paste,
  redo,
  seek,
  selectAll,
  split,
  stepFrames,
  togglePlay,
  undo,
  useEditor,
} from "./store";

interface Actions {
  /** A dialog is open, and has the keys. */
  busy: boolean;
  /** Runs something that can fail, showing why if it does. */
  attempt: (work: () => Promise<unknown>) => void;
  openExport: () => void;
  openSettings: () => void;
}

export function useShortcuts(actions: Actions) {
  const latest = useRef(actions);
  latest.current = actions;

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const { busy, attempt, openExport, openSettings } = latest.current;
      if (busy) return;
      if (e.target instanceof Element && e.target.closest("input, textarea, select, dialog")) return;
      const s = useEditor.getState();
      if (e.ctrlKey || e.metaKey) {
        switch (e.key.toLowerCase()) {
          case "z":
            if (e.shiftKey) redo();
            else undo();
            break;
          case "y":
            redo();
            break;
          case "c":
            copySelection();
            break;
          case "x":
            cutSelection();
            break;
          case "v":
            paste();
            break;
          case "a":
            selectAll();
            break;
          case "s":
            attempt(() => saveProject(e.shiftKey));
            break;
          case "o":
            attempt(chooseProject);
            break;
          case "n":
            attempt(startNewProject);
            break;
          case "i":
            attempt(importMedia);
            break;
          case "e":
            if (s.clips.length) openExport();
            break;
          case ",":
            openSettings();
            break;
          default:
            return;
        }
      } else {
        switch (e.key) {
          case " ":
          case "k":
            togglePlay();
            break;
          case "ArrowLeft":
            if (e.shiftKey) seek(s.playhead - 1);
            else stepFrames(-1);
            break;
          case "ArrowRight":
            if (e.shiftKey) seek(s.playhead + 1);
            else stepFrames(1);
            break;
          case "Home":
            seek(0);
            break;
          case "End":
            seek(totalLength(s.clips));
            break;
          case "s":
            split();
            break;
          case "f":
            toggleFullscreen();
            break;
          case "Delete":
          case "Backspace":
            deleteSelected();
            break;
          case "Escape":
            if (useFullscreen.getState().on) setFullscreen(false);
            else clearSelection();
            break;
          default:
            return;
        }
      }
      e.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
}

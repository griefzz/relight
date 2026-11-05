import { useCallback, useEffect, useRef, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { message } from "@tauri-apps/plugin-dialog";
import {
  initialFile,
  isExporting,
  onOpenFile,
  onQuitRequested,
  onRecorded,
  onRecorderError,
  onReplaySaved,
  openMedia,
  quitApp,
  takeEditor,
  takeNotice,
  unloadToTray,
  windowReady,
  type Recorded,
} from "./api";
import { clipLength, totalLength } from "./editor/clips";
import { chooseProject, okToDiscard, openPaths, saveProject, startNewProject } from "./editor/files";
import { useShortcuts } from "./editor/shortcuts";
import { addSavedReplay, restoreEditor, seek, stashEditor, useEditor } from "./editor/store";
import { ExportDialog } from "./export/ExportDialog";
import { Viewer } from "./player/Viewer";
import { RecordButton } from "./recording/RecordButton";
import { followRecorder, useRecording } from "./recording/recorder";
import { ReplayButtons } from "./recording/ReplayButtons";
import { SettingsDialog } from "./recording/SettingsDialog";
import { MediaBin } from "./timeline/MediaBin";
import { requestPreviews } from "./timeline/previews";
import { Timeline } from "./timeline/Timeline";
import { fileName } from "./ui/format";
import { GearIcon } from "./ui/icons";
import { UpdateButton } from "./updates/UpdateButton";
import { followUpdates, useUpdate } from "./updates/updates";

/** Something to leave running in the background: what to wait for before closing. */
async function stillGoing() {
  if (await isExporting()) return "An export is still running. Cancel it first, then close.";
  const { status, replay } = useRecording.getState();
  if (status.state !== "idle") return "A recording is still going. Stop it first, then close.";
  if (replay.state === "on" && replay.saving) return "A replay is still being saved. Wait for it, then close.";
  return null;
}

/**
 * Closed with "keep running" on: the page unloads, as it takes more memory than the rest of
 * relight together, and the editor's kept to pick up in the next one. What can't carry over
 * keeps it loaded, just hidden: a dialog (`busy`), an export it's showing, an update it's
 * holding ready to install.
 */
async function leaveForTray(busy: boolean) {
  if (busy || useUpdate.getState().ready || (await isExporting())) {
    await getCurrentWindow().hide();
    return;
  }
  await unloadToTray(stashEditor(), useEditor.getState().dirty).catch(() => getCurrentWindow().hide());
}

export default function App() {
  const hasClips = useEditor((s) => s.clips.length > 0);
  const projectPath = useEditor((s) => s.projectPath);
  const dirty = useEditor((s) => s.dirty);
  const recording = useRecording((s) => s.status.state === "recording");
  const [error, setError] = useState<string | null>(null);
  const [dragging, setDragging] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [showSettings, setShowSettings] = useState(false);
  const busy = useRef(false);
  busy.current = exporting || showSettings;

  const report = useCallback((e: unknown) => setError(e instanceof Error ? e.message : String(e)), []);
  const attempt = useCallback(
    (work: () => Promise<unknown>) => {
      setError(null);
      work().catch(report);
    },
    [report],
  );

  useShortcuts({
    busy: exporting || showSettings,
    attempt,
    openExport: () => setExporting(true),
    openSettings: () => setShowSettings(true),
  });

  const name = projectPath ? fileName(projectPath).replace(/\.relight$/i, "") : "Untitled";

  useEffect(() => {
    getCurrentWindow()
      .setTitle(`${recording ? "● Recording · " : ""}${name}${dirty ? " •" : ""} – relight`)
      .catch(() => {});
  }, [name, dirty, recording]);

  // What the app has to tell the window, as it happens or from while there was no window.
  useEffect(() => {
    // A finished recording goes on the end of the timeline, with the playhead at its start.
    const recorded = (done: Recorded) =>
      attempt(async () => {
        await openPaths([done.path]);
        const { clips } = useEditor.getState();
        seek(totalLength(clips) - clipLength(clips[clips.length - 1]));
        if (done.warnings.length) throw new Error(`Recorded, but:\n${done.warnings.join("\n")}`);
      });
    // A saved replay waits in the bin: it may be one of many, saved from inside a game. It
    // leaves any error showing, as nobody's been looking at the window to have seen it.
    const replaySaved = (saved: Recorded) =>
      openMedia(saved.path)
        .then((info) => {
          addSavedReplay(info);
          if (saved.warnings.length) report(`Replay saved, but:\n${saved.warnings.join("\n")}`);
        })
        .catch(report);
    const openFile = (path: string) => attempt(() => openPaths([path]));
    const quitRequested = async () => {
      const going = await isExporting();
      if (going) {
        await message("An export is still running. Cancel it first, then quit.", { title: "Exporting", kind: "info" });
      } else if (await okToDiscard()) {
        await quitApp();
      }
    };

    const stops = [
      followRecorder(),
      onRecorded(recorded),
      onReplaySaved(replaySaved),
      onRecorderError(report),
      onOpenFile(openFile),
      onQuitRequested(quitRequested),
    ];

    // In this order: the editor as the last window left it, then what's happened since, so
    // that goes on top of it, and only once it's being listened for.
    const takeUp = async () => {
      const editor = await takeEditor();
      if (editor) restoreEditor(editor);
      await Promise.all(stops);
      const errors: string[] = [];
      let quit = false;
      for (const letter of await windowReady()) {
        if (letter.event === "recorded") recorded(letter.payload);
        else if (letter.event === "replay-saved") replaySaved(letter.payload);
        else if (letter.event === "open-file") openFile(letter.payload);
        else if (letter.event === "recorder-error") errors.push(letter.payload);
        else quit = true;
      }
      // Last, so nothing that clears errors as it starts takes these with it.
      if (errors.length) report(errors.join("\n"));
      if (quit) await quitRequested();
    };
    takeUp().catch(report);

    takeNotice().then((notice) => notice && report(notice));
    return () => {
      for (const stop of stops) stop.then((unlisten) => unlisten());
    };
  }, [attempt, report]);

  useEffect(() => followUpdates(), []);

  // Every video in the bin gets thumbnails and a waveform, made in the background.
  useEffect(() => {
    requestPreviews(Object.keys(useEditor.getState().media));
    return useEditor.subscribe((s, before) => {
      if (s.media !== before.media) requestPreviews(Object.keys(s.media));
    });
  }, []);

  useEffect(() => {
    attempt(async () => {
      const path = await initialFile();
      if (path) await openPaths([path]);
    });
  }, [attempt]);

  useEffect(() => {
    const unlisten = getCurrentWebview().onDragDropEvent(({ payload }) => {
      if (busy.current) return;
      if (payload.type === "enter" || payload.type === "over") {
        setDragging(true);
      } else {
        setDragging(false);
        if (payload.type === "drop" && payload.paths.length) attempt(() => openPaths(payload.paths));
      }
    });
    return () => {
      unlisten.then((stop) => stop());
    };
  }, [attempt]);

  useEffect(() => {
    const unlisten = getCurrentWindow().onCloseRequested(async (event) => {
      // Kept running, the window goes to the tray, and everything carries on.
      if (useRecording.getState().settings?.keepRunning) {
        event.preventDefault();
        await leaveForTray(busy.current);
        return;
      }
      const going = await stillGoing();
      if (going) {
        event.preventDefault();
        await message(going, { title: "Still going", kind: "info" });
      } else if (!(await okToDiscard())) {
        event.preventDefault();
      }
    });
    return () => {
      unlisten.then((stop) => stop());
    };
  }, []);

  return (
    <div className="app">
      <header className="topbar">
        <span className="brand">
          re<b>light</b>
        </span>
        <div className="menu">
          <button onClick={() => attempt(startNewProject)} title="New project (Ctrl+N)">
            New
          </button>
          <button onClick={() => attempt(chooseProject)} title="Open a project (Ctrl+O)">
            Open…
          </button>
          <button onClick={() => attempt(() => saveProject())} title="Save (Ctrl+S, or Ctrl+Shift+S to save as)">
            Save
          </button>
        </div>
        <span className="project-name" title={projectPath ?? "Not saved yet"}>
          {name}
          {dirty && <span className="dirty"> •</span>}
        </span>
        <div className="spacer" />
        <UpdateButton onError={report} />
        <ReplayButtons />
        <RecordButton />
        <button className="icon" onClick={() => setShowSettings(true)} title="Settings (Ctrl+,)" aria-label="Settings">
          <GearIcon />
        </button>
        <button className="primary" disabled={!hasClips} onClick={() => setExporting(true)} title="Export (Ctrl+E)">
          Export
        </button>
      </header>

      {error && (
        <div className="error-banner" role="alert">
          <span>{error}</span>
          <button onClick={() => setError(null)}>Dismiss</button>
        </div>
      )}

      <div className="workspace">
        <MediaBin onError={report} />
        <Viewer onError={report} />
      </div>
      <Timeline />

      {dragging && <div className="drop-overlay">Drop to add to the timeline</div>}
      {exporting && <ExportDialog onClose={() => setExporting(false)} />}
      {showSettings && <SettingsDialog onClose={() => setShowSettings(false)} />}
    </div>
  );
}

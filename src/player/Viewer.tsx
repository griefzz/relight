import { useEffect, useRef, useState } from "react";
import { Engine } from "./engine";
import { importMedia } from "../editor/files";
import { fileName, formatLength, formatTime } from "../ui/format";
import { PauseIcon, PlayIcon } from "../ui/icons";
import { playerTime, setPlaying, togglePlay, useEditor } from "../editor/store";
import { totalLength } from "../editor/clips";

export function Viewer({ onError }: { onError: (error: unknown) => void }) {
  const a = useRef<HTMLVideoElement>(null);
  const b = useRef<HTMLVideoElement>(null);
  const engine = useRef<Engine | null>(null);
  const clips = useEditor((s) => s.clips);
  const seeks = useEditor((s) => s.seeks);
  const playing = useEditor((s) => s.playing);
  const [unplayable, setUnplayable] = useState<string | null>(null);

  useEffect(() => {
    const player = new Engine([a.current!, b.current!], {
      time: playerTime,
      ended: () => setPlaying(false),
      unplayable: (path) => {
        setUnplayable(path);
        // It would sit there "playing" a clip that can't move; stop, so it's clear.
        if (path) setPlaying(false);
      },
    });
    engine.current = player;
    return () => player.destroy();
  }, []);

  // In this order: the clips, then where to be in them, then whether to play.
  useEffect(() => {
    engine.current?.setClips(clips);
    engine.current?.seek(useEditor.getState().playhead);
  }, [clips]);

  useEffect(() => {
    engine.current?.seek(useEditor.getState().playhead);
  }, [seeks]);

  useEffect(() => {
    if (playing) engine.current?.play();
    else engine.current?.pause();
  }, [playing]);

  return (
    <section className="viewer">
      <div className="screen" onClick={togglePlay}>
        <video ref={a} preload="auto" />
        <video ref={b} preload="auto" />
        {unplayable && (
          <div className="screen-message">
            The player can't show {fileName(unplayable)}. It can still be exported.
          </div>
        )}
        {!clips.length && <Welcome onImport={() => importMedia().catch(onError)} />}
      </div>
      <div className="transport">
        <button className="icon" onClick={togglePlay} disabled={!clips.length} title={playing ? "Pause (Space)" : "Play (Space)"}>
          {playing ? <PauseIcon /> : <PlayIcon />}
        </button>
        <Timecode />
      </div>
    </section>
  );
}

function Timecode() {
  const playhead = useEditor((s) => s.playhead);
  const total = useEditor((s) => totalLength(s.clips));
  return (
    <span className="timecode">
      {formatTime(playhead)} <span className="muted">/ {formatLength(total)}</span>
    </span>
  );
}

function Welcome({ onImport }: { onImport: () => void }) {
  return (
    <div className="welcome" onClick={(e) => e.stopPropagation()}>
      <h1>Drop videos here</h1>
      <p>or</p>
      <button className="primary" onClick={onImport}>
        Import videos…
      </button>
      <p className="hint">
        From a terminal, <code>relight clip.mp4</code> opens one straight away.
      </p>
    </div>
  );
}

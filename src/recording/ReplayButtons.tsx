import { saveReplay, startReplay, stopReplay } from "../api";
import { formatLength } from "../ui/format";
import { useRecording } from "./recorder";

/**
 * The top bar's instant replay controls: start it; once it's on, save what it holds or stop
 * it. A failure comes back as a `recorder-error`, like one from the hotkeys.
 */
export function ReplayButtons() {
  const replay = useRecording((s) => s.replay);
  const settings = useRecording((s) => s.settings);
  const length = formatLength(settings?.replaySeconds ?? 30);
  const stop = (
    <button onClick={() => stopReplay()} title={`Stop instant replay (${settings?.replayStopHotkey}, from anywhere)`}>
      Stop
    </button>
  );

  if (replay.state === "starting") return <button disabled>Starting replay…</button>;
  if (replay.state === "paused") {
    return (
      <button onClick={() => stopReplay()} title="Instant replay starts afresh when this recording's done. Click to leave it off.">
        Replay after recording
      </button>
    );
  }
  if (replay.state === "on") {
    return (
      <div className="replay-buttons">
        {replay.interrupted ? (
          <button disabled title="The screen capture stopped, and is starting again. What was held has gone.">
            Replay reconnecting…
          </button>
        ) : (
          <button
            className="replaying"
            disabled={replay.saving}
            onClick={() => saveReplay()}
            title={`Save the last ${length} (${settings?.replaySaveHotkey}, from anywhere)`}
          >
            <span className="replay-dot" />
            {replay.saving ? "Saving replay…" : "Save replay"}
          </button>
        )}
        {stop}
      </div>
    );
  }
  return (
    <button
      onClick={() => startReplay()}
      title={`Keep the last ${length} of the screen ready to save (${settings?.replayStartHotkey}, from anywhere)`}
    >
      Replay
    </button>
  );
}

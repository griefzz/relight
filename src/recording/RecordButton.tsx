import { useEffect, useState } from "react";
import { toggleRecording } from "../api";
import { formatRuler } from "../ui/format";
import { useRecording } from "./recorder";

/** The top bar's record button: starts and stops recording, and shows for how long. */
export function RecordButton() {
  const status = useRecording((s) => s.status);
  const hotkey = useRecording((s) => s.settings?.hotkey ?? "");
  if (status.state === "starting") return <button disabled>Starting…</button>;
  if (status.state === "saving") return <button disabled>Saving recording…</button>;
  if (status.state === "recording") {
    return (
      <button className="recording" onClick={() => toggleRecording()} title={`Stop recording (${hotkey})`}>
        <span className="rec-dot" />
        <Elapsed since={status.since} />
        Stop
      </button>
    );
  }
  return (
    <button onClick={() => toggleRecording()} title={`Record the screen the mouse is on (${hotkey}, from anywhere)`}>
      <span className="rec-dot" />
      Record
    </button>
  );
}

function Elapsed({ since }: { since: number }) {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    const tick = setInterval(() => setNow(Date.now()), 250);
    return () => clearInterval(tick);
  }, []);
  return <span className="timecode">{formatRuler(Math.max(0, now - since) / 1000, 1)}</span>;
}

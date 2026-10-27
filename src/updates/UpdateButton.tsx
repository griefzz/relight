import { useState } from "react";
import { okToDiscard } from "../editor/files";
import { useRecording } from "../recording/recorder";
import { installUpdate, useUpdate } from "./updates";

/** Shows once a newer version is downloaded: installs it and restarts, when nothing's lost by it. */
export function UpdateButton({ onError }: { onError: (error: unknown) => void }) {
  const update = useUpdate((s) => s.ready);
  const recording = useRecording((s) => s.status.state !== "idle");
  const [installing, setInstalling] = useState(false);
  if (!update) return null;

  const install = async () => {
    if (!(await okToDiscard())) return;
    setInstalling(true);
    try {
      await installUpdate();
    } catch (e) {
      setInstalling(false);
      onError(e);
    }
  };

  return (
    <button
      className="update"
      disabled={recording || installing}
      onClick={install}
      title={recording ? "Once the recording is saved" : `Install relight ${update.version} and restart`}
    >
      {installing ? "Updating…" : `Update to ${update.version}`}
    </button>
  );
}

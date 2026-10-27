import { useEffect, useRef, useState } from "react";
import { save } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import {
  CANCELLED,
  cancelExport,
  exportTimeline,
  onExportReport,
  type Exported,
  type Picture,
  type Preset,
  type Tune,
} from "../api";
import { Segmented } from "../ui/controls";
import { fileName, formatLength, formatMB, outputPath } from "../ui/format";
import { useEditor } from "../editor/store";
import { totalLength } from "../editor/clips";

interface Settings {
  kind: "full" | "discord";
  targetMb: number;
  av1: boolean;
  tune: Tune;
  /** The short side ("720"), or "same" as the first clip. */
  size: string;
  /** Frames a second ("30"), or "same" as the first clip. */
  fps: string;
}

const DEFAULTS: Settings = { kind: "discord", targetMb: 20, av1: false, tune: "animation", size: "same", fps: "same" };

const SIZES = ["1080", "720", "480"];
const RATES = ["60", "30"];
const SETTINGS_KEY = "relight.export";

// The last settings used, so the next export starts from them.
function loadSettings(): Settings {
  try {
    return { ...DEFAULTS, ...JSON.parse(localStorage.getItem(SETTINGS_KEY) ?? "{}") };
  } catch {
    return DEFAULTS;
  }
}

function saveSettings(settings: Settings) {
  try {
    localStorage.setItem(SETTINGS_KEY, JSON.stringify(settings));
  } catch {
    // Not remembering them is fine.
  }
}

function toPreset(s: Settings): Preset {
  return s.kind === "full" ? { kind: "full" } : { kind: "discord", targetMb: s.targetMb, av1: s.av1, tune: s.tune };
}

type Phase =
  | { name: "options" }
  | { name: "running"; stage: string; fraction: number; notes: string[] }
  | { name: "done"; result: Exported; preset: Preset }
  | { name: "failed"; error: string };

export function ExportDialog({ onClose }: { onClose: () => void }) {
  const clips = useEditor((s) => s.clips);
  const projectPath = useEditor((s) => s.projectPath);
  const first = useEditor((s) => s.media[s.clips[0]?.path]?.video ?? null);
  const dialog = useRef<HTMLDialogElement>(null);
  const [settings, setSettings] = useState(loadSettings);
  const [phase, setPhase] = useState<Phase>({ name: "options" });

  // Only smaller than the first clip: it's never scaled up or given made-up frames. A
  // remembered choice that doesn't fit this timeline reads as "same".
  const sizes = first ? SIZES.filter((size) => Number(size) < Math.min(first.width, first.height)) : [];
  const rates = first ? RATES.filter((fps) => Number(fps) < first.fps - 0.5) : [];
  const size = sizes.includes(settings.size) ? settings.size : "same";
  const fps = rates.includes(settings.fps) ? settings.fps : "same";
  const picture: Picture = {
    shortSide: size === "same" ? null : Number(size),
    fps: fps === "same" ? null : Number(fps),
  };

  useEffect(() => {
    const d = dialog.current;
    if (d && !d.open) d.showModal();
  }, []);

  const update = (patch: Partial<Settings>) =>
    setSettings((s) => {
      const next = { ...s, ...patch };
      saveSettings(next);
      return next;
    });

  const start = async () => {
    const preset = toPreset(settings);
    const chosen = await save({
      defaultPath: outputPath(projectPath ?? clips[0].path, preset.kind === "discord" ? "_discord" : "_full"),
      filters: [{ name: "MP4 video", extensions: ["mp4"] }],
    });
    if (!chosen) return;
    // It's an MP4 whatever it's called, so it's called that.
    const output = /\.mp4$/i.test(chosen) ? chosen : `${chosen}.mp4`;

    setPhase({ name: "running", stage: "Starting", fraction: 0, notes: [] });
    const unlisten = await onExportReport((report) =>
      setPhase((p) => {
        if (p.name !== "running") return p;
        switch (report.type) {
          case "stage":
            return { ...p, stage: report.label, fraction: 0 };
          case "progress":
            return { ...p, fraction: report.fraction };
          case "note":
            return { ...p, notes: [...p.notes, report.text] };
        }
      }),
    );
    try {
      const segments = clips.map(({ path, start, end }) => ({ path, start, end }));
      setPhase({ name: "done", result: await exportTimeline(segments, picture, output, preset), preset });
    } catch (e) {
      setPhase(String(e) === CANCELLED ? { name: "options" } : { name: "failed", error: String(e) });
    } finally {
      unlisten();
    }
  };

  return (
    <dialog
      ref={dialog}
      className="dialog"
      onCancel={(e) => {
        e.preventDefault();
        if (phase.name !== "running") onClose();
      }}
    >
      {phase.name === "options" && (
        <>
          <h2>Export</h2>
          <p className="muted">
            {clips.length === 1 ? fileName(clips[0].path) : `${clips.length} clips`} · {formatLength(totalLength(clips))}
          </p>

          <div className="presets" role="radiogroup" aria-label="Preset">
            <PresetCard
              title="Full quality"
              description="H.264 at visually lossless quality, shaped like the first clip."
              selected={settings.kind === "full"}
              onSelect={() => update({ kind: "full" })}
            />
            <PresetCard
              title="Discord"
              description="Fits under the upload limit, at the best quality that allows."
              selected={settings.kind === "discord"}
              onSelect={() => update({ kind: "discord" })}
            />
          </div>

          {(sizes.length > 0 || rates.length > 0 || settings.kind === "discord") && (
            <div className="options">
              {first && sizes.length > 0 && (
                <Segmented
                  label="Size"
                  value={size}
                  choices={[["same", `${first.width}×${first.height}`], ...sizes.map((s): [string, string] => [s, `${s}p`])]}
                  onChange={(size) => update({ size })}
                  hint={settings.kind === "discord" ? "The most it'll be: it goes smaller when that looks better." : undefined}
                />
              )}
              {first && rates.length > 0 && (
                <Segmented
                  label="Frame rate"
                  value={fps}
                  choices={[["same", `${Math.round(first.fps)} fps`], ...rates.map((f): [string, string] => [f, `${f} fps`])]}
                  onChange={(fps) => update({ fps })}
                />
              )}
              {settings.kind === "discord" && <DiscordOptions settings={settings} update={update} />}
            </div>
          )}

          <footer>
            <button onClick={onClose}>Cancel</button>
            <button
              className="primary"
              onClick={start}
              disabled={settings.kind === "discord" && !(settings.targetMb > 0)}
            >
              Export…
            </button>
          </footer>
        </>
      )}

      {phase.name === "running" && (
        <>
          <h2>Exporting</h2>
          <div className="progress-line">
            <span>{phase.stage}</span>
            <span className="muted">{Math.round(phase.fraction * 100)}%</span>
          </div>
          <div className="progress">
            <div style={{ width: `${phase.fraction * 100}%` }} />
          </div>
          {phase.notes.length > 0 && (
            <div className="notes">
              {phase.notes.map((note, i) => (
                <p key={i}>{note}</p>
              ))}
            </div>
          )}
          <footer>
            <button onClick={() => cancelExport()}>Cancel export</button>
          </footer>
        </>
      )}

      {phase.name === "done" && (
        <>
          <h2>Exported</h2>
          <p>
            <strong>{fileName(phase.result.path)}</strong>
            <span className="muted">
              {" "}
              · {formatMB(phase.result.bytes)}
              {phase.preset.kind === "discord" && `, under the ${phase.preset.targetMb} MB limit`}
            </span>
          </p>
          <footer>
            <button onClick={() => revealItemInDir(phase.result.path)}>Show in folder</button>
            <button className="primary" onClick={onClose}>
              Done
            </button>
          </footer>
        </>
      )}

      {phase.name === "failed" && (
        <>
          <h2>Export failed</h2>
          <pre className="error-text">{phase.error}</pre>
          <footer>
            <button onClick={() => setPhase({ name: "options" })}>Back</button>
            <button onClick={onClose}>Close</button>
          </footer>
        </>
      )}
    </dialog>
  );
}

function DiscordOptions({ settings, update }: { settings: Settings; update: (patch: Partial<Settings>) => void }) {
  return (
    <>
      <label className="option">
        <span className="option-label">Size limit</span>
        <span className="mb-input">
          <input
            type="number"
            min={1}
            step={1}
            value={settings.targetMb || ""}
            onChange={(e) => update({ targetMb: Number(e.target.value) })}
          />
          MB
        </span>
      </label>
      <Segmented
        label="Codec"
        value={settings.av1 ? "av1" : "h264"}
        choices={[
          ["h264", "H.264"],
          ["av1", "AV1"],
        ]}
        onChange={(codec) => update({ av1: codec === "av1" })}
        hint={
          settings.av1
            ? "Looks better on long clips, but older phones can't play it."
            : "Plays inline on every Discord client."
        }
      />
      {!settings.av1 && (
        <Segmented
          label="Footage"
          value={settings.tune}
          choices={[
            ["animation", "Game / animation"],
            ["film", "Camera"],
          ]}
          onChange={(tune) => update({ tune })}
        />
      )}
    </>
  );
}

function PresetCard(props: { title: string; description: string; selected: boolean; onSelect: () => void }) {
  return (
    <button
      role="radio"
      aria-checked={props.selected}
      className={`preset${props.selected ? " selected" : ""}`}
      onClick={props.onSelect}
    >
      <strong>{props.title}</strong>
      <span>{props.description}</span>
    </button>
  );
}

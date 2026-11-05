import { useEffect, useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from "react";
import { join, videoDir } from "@tauri-apps/api/path";
import { open } from "@tauri-apps/plugin-dialog";
import { hotkeyWorking, pauseHotkey, recordingSources, type Screen, type Settings, type Sources } from "../api";
import { Option, Segmented } from "../ui/controls";
import { changeSettings, replayDisk, replayMemory, useRecording } from "./recorder";

/** How long instant replay can hold, in seconds. */
const REPLAY_LENGTHS: [number, string][] = [
  [15, "15 seconds"],
  [30, "30 seconds"],
  [60, "1 minute"],
  [120, "2 minutes"],
  [300, "5 minutes"],
];

export function SettingsDialog({ onClose }: { onClose: () => void }) {
  const dialog = useRef<HTMLDialogElement>(null);
  const done = useRef<HTMLButtonElement>(null);
  const settings = useRecording((s) => s.settings);
  const [sources, setSources] = useState<Sources | null>(null);
  const [defaultFolder, setDefaultFolder] = useState("");
  const [problem, setProblem] = useState<string | null>(null);
  /** The hotkeys that another program has. */
  const [taken, setTaken] = useState<string[]>([]);
  const checkHotkeys = async () => {
    const now = useRecording.getState().settings;
    if (!now) return;
    const hotkeys = [now.hotkey, now.replayStartHotkey, now.replayStopHotkey, now.replaySaveHotkey];
    const working = await Promise.all(hotkeys.map((hotkey) => hotkeyWorking(hotkey).catch(() => true)));
    setTaken(hotkeys.filter((_, i) => !working[i]));
  };

  useEffect(() => {
    const d = dialog.current;
    if (d && !d.open) d.showModal();
    // Not the hotkey field, which would start listening for a new hotkey straight away.
    done.current?.focus();
    checkHotkeys();
    recordingSources().then(setSources, (e) => setProblem(`Couldn't list the screens and sound devices: ${e}`));
    videoDir()
      .then((videos) => join(videos, "relight"))
      .then(setDefaultFolder)
      .catch(() => {});
  }, []);

  const change = async (patch: Partial<Settings>) => {
    if (!settings) return;
    setProblem(null);
    try {
      await changeSettings({ ...settings, ...patch });
    } catch (e) {
      setProblem(String(e));
    }
    checkHotkeys();
  };

  /** A hotkey's field, in a row of its own. */
  const hotkeyRow = (label: string, hint: string, hotkey: string, patch: (hotkey: string) => Partial<Settings>) => (
    <Option label={label} hint={hint}>
      <HotkeyField
        value={hotkey}
        working={!taken.includes(hotkey)}
        onChange={(next) => change(patch(next))}
        onResume={checkHotkeys}
      />
    </Option>
  );

  const pickFolder = async () => {
    const folder = await open({ directory: true, defaultPath: settings?.folder ?? defaultFolder });
    if (folder) change({ folder });
  };

  return (
    <dialog
      ref={dialog}
      className="dialog settings"
      onCancel={(e) => {
        e.preventDefault();
        onClose();
      }}
    >
      <h2>Settings</h2>
      {settings && (
        <>
          <h3>Recording</h3>
          <div className="options">
            {hotkeyRow("Hotkey", "Starts and stops recording from anywhere, games included.", settings.hotkey, (hotkey) => ({
              hotkey,
            }))}
            <Option label="Screen">
              <select
                value={settings.screen ?? ""}
                onChange={(e) => change({ screen: e.target.value === "" ? null : Number(e.target.value) })}
              >
                <option value="">The one the mouse is on</option>
                {sources?.screens.map((screen, i) => (
                  <option key={screen.index} value={screen.index}>
                    {describeScreen(screen, i)}
                  </option>
                ))}
              </select>
            </Option>
            <Segmented
              label="Frame rate"
              value={String(settings.fps)}
              choices={[
                ["60", "60 fps"],
                ["30", "30 fps"],
              ]}
              onChange={(fps) => change({ fps: Number(fps) })}
            />
            <Option label="Game sound">
              <label className="check">
                <input
                  type="checkbox"
                  checked={settings.gameSound}
                  onChange={(e) => change({ gameSound: e.target.checked })}
                />
                Record what plays through
              </label>
              <DeviceSelect
                value={settings.gameDevice}
                devices={sources?.sound.outputs}
                fallback={sources?.sound.defaultOutput}
                disabled={!settings.gameSound}
                onChange={(gameDevice) => change({ gameDevice })}
              />
            </Option>
            <Option label="Microphone">
              <label className="check">
                <input
                  type="checkbox"
                  checked={settings.microphone}
                  onChange={(e) => change({ microphone: e.target.checked })}
                />
                Record
              </label>
              <DeviceSelect
                value={settings.micDevice}
                devices={sources?.sound.inputs}
                fallback={sources?.sound.defaultInput}
                disabled={!settings.microphone}
                onChange={(micDevice) => change({ micDevice })}
              />
            </Option>
            <Option label="Save to">
              <div className="folder">
                <span title={settings.folder ?? defaultFolder}>{settings.folder ?? defaultFolder}</span>
                <button onClick={pickFolder}>Change…</button>
              </div>
            </Option>
          </div>

          <h3>Instant replay</h3>
          <p className="hint">
            Keeps the last stretch of the screen {settings.replayBounce ? "on disk" : "in memory"}, ready to save. It uses the
            screen, frame rate, game sound and folder above. Recording stops it and clears what it holds; it starts afresh when
            the recording's done.
          </p>
          <div className="options">
            {hotkeyRow("Start", "Starts it from anywhere.", settings.replayStartHotkey, (replayStartHotkey) => ({
              replayStartHotkey,
            }))}
            {hotkeyRow("Stop", "Stops it from anywhere.", settings.replayStopHotkey, (replayStopHotkey) => ({
              replayStopHotkey,
            }))}
            {hotkeyRow("Save", "Saves what it holds from anywhere.", settings.replaySaveHotkey, (replaySaveHotkey) => ({
              replaySaveHotkey,
            }))}
            <Option
              label="Length"
              hint={`Uses up to about ${formatSize(
                replayMemory(settings.replaySeconds, settings.gameSound, settings.replayMicrophone, settings.replayBounce),
              )} of memory${
                settings.replayBounce ? ` and ${formatSize(replayDisk(settings.replaySeconds))} of disk space` : ""
              } while it's on.`}
            >
              <select value={settings.replaySeconds} onChange={(e) => change({ replaySeconds: Number(e.target.value) })}>
                {REPLAY_LENGTHS.map(([seconds, label]) => (
                  <option key={seconds} value={seconds}>
                    {label}
                  </option>
                ))}
                {!REPLAY_LENGTHS.some(([seconds]) => seconds === settings.replaySeconds) && (
                  <option value={settings.replaySeconds}>{settings.replaySeconds} seconds</option>
                )}
              </select>
            </Option>
            <Option label="Microphone" hint="Off by default: it would be listening for as long as replay is on.">
              <label className="check">
                <input
                  type="checkbox"
                  checked={settings.replayMicrophone}
                  onChange={(e) => change({ replayMicrophone: e.target.checked })}
                />
                Include the microphone
              </label>
            </Option>
            <Option
              label="Bounce to disk"
              hint="For long game sessions: frees most of its memory for your game. The picture goes to temporary files as it's captured, so the disk is written to for as long as replay is on. The sound stays in memory."
            >
              <label className="check">
                <input
                  type="checkbox"
                  checked={settings.replayBounce}
                  onChange={(e) => change({ replayBounce: e.target.checked })}
                />
                Keep the picture on disk
              </label>
            </Option>
            <Option label="Sound alert" hint="A lower one if it couldn't be saved. Other replays reaching back that far may pick it up.">
              <label className="check">
                <input
                  type="checkbox"
                  checked={settings.replayChime}
                  onChange={(e) => change({ replayChime: e.target.checked })}
                />
                Play a sound when saving a replay
              </label>
            </Option>
          </div>

          <h3>Window</h3>
          <label className="check">
            <input
              type="checkbox"
              checked={settings.keepRunning}
              onChange={(e) => change({ keepRunning: e.target.checked })}
            />
            Keep running in the tray when the window is closed, so the hotkeys still work
          </label>
          <p className="hint">The window is unloaded while it's closed, to free its memory, and comes back as you left it.</p>
        </>
      )}
      {problem && <pre className="error-text">{problem}</pre>}
      <footer>
        <button ref={done} className="primary" onClick={onClose}>
          Done
        </button>
      </footer>
    </dialog>
  );
}

function formatSize(megabytes: number) {
  return megabytes < 1000 ? `${megabytes} MB` : `${(megabytes / 1000).toFixed(1)} GB`;
}

function describeScreen(screen: Screen, i: number) {
  const where = screen.primary
    ? "main"
    : screen.x < 0
      ? "left of the main one"
      : screen.x > 0
        ? "right of the main one"
        : screen.y < 0
          ? "above the main one"
          : "below the main one";
  return `Screen ${i + 1}: ${screen.width}×${screen.height}, ${where}`;
}

function DeviceSelect(props: {
  value: string | null;
  devices: string[] | undefined;
  fallback: string | null | undefined;
  disabled: boolean;
  onChange: (device: string | null) => void;
}) {
  const devices = props.devices ?? [];
  return (
    <select value={props.value ?? ""} disabled={props.disabled} onChange={(e) => props.onChange(e.target.value || null)}>
      <option value="">Default{props.fallback ? ` (${props.fallback})` : ""}</option>
      {devices.map((name) => (
        <option key={name} value={name}>
          {name}
        </option>
      ))}
      {props.value && !devices.includes(props.value) && <option value={props.value}>{props.value} (not connected)</option>}
    </select>
  );
}

const MODIFIERS = ["Control", "Shift", "Alt", "Meta"];
const OTHER_KEYS = [
  "Backquote", "Minus", "Equal", "BracketLeft", "BracketRight", "Backslash", "Semicolon", "Quote",
  "Comma", "Period", "Slash", "Space", "Insert", "Delete", "Home", "End", "PageUp", "PageDown",
  "Pause", "ScrollLock", "PrintScreen",
];

/** A key's name as the hotkey reads it: "5", "R", "F9", "PageUp". */
function keyName(code: string, key: string): string | null {
  if (/^Digit\d$/.test(code)) return code.slice(5);
  if (/^Key[A-Z]$/.test(code)) return code.slice(3);
  if (/^F\d{1,2}$/.test(code) || /^(Numpad|Arrow)/.test(code) || OTHER_KEYS.includes(code)) return code;
  // Keys pressed by other software can come without a code: go by the key itself.
  if (!code && /^[a-z0-9]$/i.test(key)) return key.toUpperCase();
  if (!code && /^F\d{1,2}$/.test(key)) return key;
  return null;
}

/**
 * Shows the hotkey; focused, it takes the next key combination pressed as the new one.
 * Pressing the same one again tries it again, for when another program had it.
 */
function HotkeyField(props: {
  value: string;
  /** Whether it's relight's, rather than another program's. */
  working: boolean;
  onChange: (hotkey: string) => void;
  /** The hotkeys are back on after listening. */
  onResume: () => void;
}) {
  const { value, working, onChange, onResume } = props;
  const [listening, setListening] = useState(false);
  const [hint, setHint] = useState<string | null>(null);

  // The hotkeys are let go of while listening, so pressing one here doesn't start anything.
  const paused = useRef(false);
  const pause = (on: boolean) => {
    if (paused.current === on) return;
    paused.current = on;
    pauseHotkey(on)
      .catch(() => {})
      .finally(() => !on && onResume());
  };
  // Closing the dialog while listening doesn't blur the field: take the hotkeys back then too.
  useEffect(() => () => pause(false), []);

  const press = (e: ReactKeyboardEvent<HTMLButtonElement>) => {
    e.preventDefault();
    e.stopPropagation();
    if (e.key === "Escape") return e.currentTarget.blur();
    if (MODIFIERS.includes(e.key)) return;
    const key = keyName(e.code, e.key);
    if (!key) return setHint("That key can't be a hotkey.");
    const held = [e.ctrlKey && "Ctrl", e.altKey && "Alt", e.shiftKey && "Shift", e.metaKey && "Super"].filter(
      (modifier): modifier is string => Boolean(modifier),
    );
    // On its own, a letter or number would go off whenever it's typed.
    if (!held.length && !/^F\d+$/.test(key)) return setHint("Hold Ctrl, Alt or Shift with it, or use an F key.");
    setHint(null);
    onChange([...held, key].join("+"));
    e.currentTarget.blur();
  };

  return (
    <>
      <button
        className={`hotkey${listening ? " listening" : ""}`}
        onFocus={() => {
          setListening(true);
          pause(true);
        }}
        onBlur={() => {
          setListening(false);
          pause(false);
        }}
        onKeyDown={press}
      >
        {listening ? "Press the keys…" : value}
      </button>
      {/* A combination another program has never gets this far, so nothing happens. */}
      {(hint || listening) && (
        <p className="hint">{hint ?? "If nothing happens when you press them, another program has that combination."}</p>
      )}
      {!hint && !listening && !working && (
        <p className="hint warning">
          Another program has {value}, so it does nothing. Close that program and press {value} here again, or pick other keys.
        </p>
      )}
    </>
  );
}

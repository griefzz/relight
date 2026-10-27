import { MOST_LEVEL, sourcesOf } from "../editor/clips";
import { beginDrag, changeLevel, endDrag, useEditor } from "../editor/store";

/** Shorter names for a recording's sources, where the toolbar is tight. */
const SHORT: Record<string, string> = { Microphone: "Mic" };

/**
 * How loud each of a recording's sources is ("Game", "Microphone"), for the selected clips
 * that are recordings: a slider each, from silent to twice as recorded. A drag is one
 * undo step; double-click puts a source back as recorded.
 */
export function Balance() {
  const chosen = useEditor((s) => s.clips.find((clip) => s.selection.includes(clip.id) && sourcesOf(s.media[clip.path])));
  // The file's info, not its sources: a selector has to give back the same thing each time.
  const info = useEditor((s) => (chosen ? s.media[chosen.path] : undefined));
  const sources = sourcesOf(info);
  if (!chosen || !sources) return null;

  return (
    <>
      <span className="divider" />
      <div className="balance" title="How loud each sound is in the selected clips. Double-click a slider to put it back.">
        {sources.map((name, n) => {
          const percent = Math.round((chosen.levels?.[n] ?? 1) * 100);
          return (
            <label key={name} className="level">
              <span>{SHORT[name] ?? name}</span>
              <input
                type="range"
                min={0}
                max={MOST_LEVEL * 100}
                step={1}
                value={percent}
                aria-label={`${name} volume`}
                onPointerDown={beginDrag}
                onPointerUp={(e) => {
                  endDrag();
                  e.currentTarget.blur();
                }}
                onBlur={endDrag}
                onChange={(e) => changeLevel(n, Number(e.target.value) / 100)}
                onDoubleClick={() => changeLevel(n, 1)}
              />
              <span className="percent">{percent}%</span>
            </label>
          );
        })}
      </div>
    </>
  );
}

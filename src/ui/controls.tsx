// Small form controls shared by the dialogs.

import type { ReactNode } from "react";

/** One of a few choices, as a row of buttons, with a label and an optional hint under it. */
export function Segmented<T extends string>(props: {
  label: string;
  value: T;
  choices: [T, string][];
  onChange: (value: T) => void;
  hint?: string;
}) {
  return (
    <Option label={props.label} hint={props.hint}>
      <div className="segmented" role="radiogroup" aria-label={props.label}>
        {props.choices.map(([value, label]) => (
          <button
            key={value}
            role="radio"
            aria-checked={props.value === value}
            className={props.value === value ? "selected" : ""}
            onClick={() => props.onChange(value)}
          >
            {label}
          </button>
        ))}
      </div>
    </Option>
  );
}

/** A labelled row of a dialog's options. */
export function Option(props: { label: string; hint?: string; children: ReactNode }) {
  return (
    <div className="option">
      <span className="option-label">{props.label}</span>
      <div>
        {props.children}
        {props.hint && <p className="hint">{props.hint}</p>}
      </div>
    </div>
  );
}

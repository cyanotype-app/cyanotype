import { useId, useState } from "react";
import type { Person } from "../api";
import { FIND_A_PERSON, matchingPeople, personLabel, photosWord } from "../peopleText";

interface Props {
  /** The people to choose from: named and not hidden (`filterablePeople`). */
  people: readonly Person[];
  /** The chosen people's ids. */
  value: readonly string[];
  onChange: (ids: string[]) => void;
  /** The group's name for assistive technology and the visible legend. */
  label: string;
  /** Most people that can be chosen (the checkboxes of the others are off once it is reached). */
  max: number;
  disabled?: boolean;
}

/** Most people the list shows at once; typing narrows it. */
const SHOWN = 8;

/** Choose several named people: a box to narrow the list, and a short list of checkboxes (the chosen ones first). */
export function PeoplePicker({ people, value, onChange, label, max, disabled = false }: Props) {
  const id = useId();
  const [typed, setTyped] = useState("");
  const list = matchingPeople(people, typed, value);
  const shown = list.slice(0, Math.max(SHOWN, value.length));
  // A chosen id that is not in the list any more (a deleted or renamed-to-nothing person) can still be taken off.
  const gone = value.filter((v) => !people.some((p) => p.id === v));
  const toggle = (pid: string, on: boolean) => onChange(on ? [...value, pid] : value.filter((v) => v !== pid));
  return (
    <fieldset className="people-picker" disabled={disabled}>
      <legend>{label}</legend>
      <label htmlFor={`${id}-find`} className="sr-only">
        {FIND_A_PERSON}
      </label>
      <input
        id={`${id}-find`}
        className="text-input"
        type="search"
        placeholder={FIND_A_PERSON}
        value={typed}
        maxLength={100}
        onChange={(e) => setTyped(e.target.value)}
      />
      <ul className="people-picker-list">
        {gone.map((g) => (
          <li key={g}>
            <label className="sheet-check">
              <input type="checkbox" checked onChange={() => toggle(g, false)} />
              <span>Unknown person</span>
            </label>
          </li>
        ))}
        {shown.map((p) => {
          const on = value.includes(p.id);
          return (
            <li key={p.id}>
              <label className="sheet-check">
                <input type="checkbox" checked={on} disabled={!on && value.length >= max} onChange={(e) => toggle(p.id, e.target.checked)} />
                <span className="people-picker-name">
                  {personLabel(p)} <span className="muted small">({photosWord(p.photos)})</span>
                </span>
              </label>
            </li>
          );
        })}
      </ul>
      {list.length > shown.length && (
        <p className="muted small" role="status">
          {list.length - shown.length} more: type a name to find them.
        </p>
      )}
      {typed.trim() !== "" && !list.some((p) => !value.includes(p.id)) && (
        <p className="muted small" role="status">
          No one is called that.
        </p>
      )}
    </fieldset>
  );
}

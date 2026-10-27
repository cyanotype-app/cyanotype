import { useId, useState } from "react";
import type { FaceBox } from "../api";
import { FIND_A_PERSON, matchingPeople, personLabel, photosWord } from "../peopleText";
import { FIND_A_PET } from "../petsText";
import { cn } from "../ui/cn";
import { BlankFace, FaceCrop } from "./FaceCrop";
import { inputClass } from "./sheetForm";

/** What the picker needs of a person or a named pet. */
export interface Pickable {
  id: string;
  name: string | null;
  photos: number;
  cover: { preview_url: string; box: FaceBox } | null;
}

interface Props {
  /** The people (or pets) to choose from: named and not hidden (`filterablePeople`, `filterablePets`). */
  people: readonly Pickable[];
  /** Choose pets instead of people: the words change (find a pet, an unknown pet), the rows are the same. */
  pets?: boolean;
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
export function PeoplePicker({ people, pets = false, value, onChange, label, max, disabled = false }: Props) {
  const id = useId();
  const [typed, setTyped] = useState("");
  const list = matchingPeople(people, typed, value);
  const find = pets ? FIND_A_PET : FIND_A_PERSON;
  const shown = list.slice(0, Math.max(SHOWN, value.length));
  // A chosen id that is not in the list any more (a deleted or renamed-to-nothing person) can still be taken off.
  const gone = value.filter((v) => !people.some((p) => p.id === v));
  const toggle = (pid: string, on: boolean) => onChange(on ? [...value, pid] : value.filter((v) => v !== pid));
  const row = "flex min-h-11 cursor-pointer items-center gap-3 rounded-[0.6rem] px-1.5 text-[0.9375rem] hover:bg-surface has-[:disabled]:cursor-default has-[:disabled]:opacity-55";
  return (
    <fieldset className="m-0 flex min-w-0 flex-col gap-2 border-0 p-0" disabled={disabled}>
      <legend className="mb-1.5 p-0 text-[0.8125rem] font-medium text-muted">{label}</legend>
      <label htmlFor={`${id}-find`} className="sr-only">
        {find}
      </label>
      <input
        id={`${id}-find`}
        className={cn(inputClass, "bg-card ring-line")}
        type="search"
        placeholder={find}
        value={typed}
        maxLength={100}
        onChange={(e) => setTyped(e.target.value)}
      />
      <ul className="flex flex-col">
        {gone.map((g) => (
          <li key={g}>
            <label className={row}>
              <input type="checkbox" className="size-5 shrink-0 accent-accent" checked onChange={() => toggle(g, false)} />
              <span>{pets ? "Unknown pet" : "Unknown person"}</span>
            </label>
          </li>
        ))}
        {shown.map((p) => {
          const on = value.includes(p.id);
          return (
            <li key={p.id}>
              <label className={row}>
                <input
                  type="checkbox"
                  className="size-5 shrink-0 accent-accent"
                  checked={on}
                  disabled={!on && value.length >= max}
                  onChange={(e) => toggle(p.id, e.target.checked)}
                />
                {p.cover ? <FaceCrop url={p.cover.preview_url} box={p.cover.box} className="size-8 shrink-0" /> : <BlankFace className="size-8 shrink-0" />}
                <span className="min-w-0 [overflow-wrap:anywhere]">
                  {pets ? p.name : personLabel(p)} <span className="text-[0.8125rem] text-muted">({photosWord(p.photos)})</span>
                </span>
              </label>
            </li>
          );
        })}
      </ul>
      {list.length > shown.length && (
        <p className="text-[0.8125rem] text-muted" role="status">
          {list.length - shown.length} more: type a name to find them.
        </p>
      )}
      {typed.trim() !== "" && !list.some((p) => !value.includes(p.id)) && (
        <p className="text-[0.8125rem] text-muted" role="status">
          {pets ? "No pet is called that." : "No one is called that."}
        </p>
      )}
    </fieldset>
  );
}

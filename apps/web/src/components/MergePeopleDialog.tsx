import { useEffect, useRef, useState, type FormEvent } from "react";
import type { Person } from "../api";
import { isNamed, nameProblem, personLabel, photosWord } from "../peopleText";
import { FaceCrop } from "./FaceCrop";

/** The person to keep by default: the named one with the most photos, else the one with the most photos. */
export const defaultSurvivor = (people: readonly Person[]): Person | undefined =>
  [...people].sort((a, b) => Number(isNamed(b)) - Number(isNamed(a)) || b.photos - a.photos)[0];

/**
 * Merge several people into one: choose who stays (their name stays, unless they have none; then the first name among the others is
 * taken), optionally type the name they end up with, and confirm. All of the others' photos go to the one who stays, and the others
 * are deleted. Calls `onMerge(into, name)`; `name` is undefined when the field is left blank.
 */
export function MergePeopleDialog({
  people,
  busy,
  error,
  onMerge,
  onCancel,
}: {
  people: readonly Person[];
  busy: boolean;
  error: string | null;
  onMerge: (into: Person, name: string | undefined) => void;
  onCancel: () => void;
}) {
  const [into, setInto] = useState(() => defaultSurvivor(people)?.id ?? "");
  const [name, setName] = useState("");
  const [problem, setProblem] = useState<string | null>(null);
  const ref = useRef<HTMLFormElement>(null);

  useEffect(() => {
    const prev = document.activeElement as HTMLElement | null;
    ref.current?.querySelector<HTMLElement>("input[type=radio]:checked")?.focus();
    return () => prev?.focus?.();
  }, []);
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        onCancel();
      }
    }
    document.addEventListener("keydown", onKey, true);
    return () => document.removeEventListener("keydown", onKey, true);
  }, [onCancel]);

  const target = people.find((p) => p.id === into);
  function submit(e: FormEvent) {
    e.preventDefault();
    if (!target) return;
    const bad = nameProblem(name);
    if (bad) return setProblem(bad);
    onMerge(target, name.trim() === "" ? undefined : name.trim());
  }
  return (
    <div className="modal-backdrop">
      <form className="modal card" role="dialog" aria-modal="true" aria-labelledby="merge-people-title" ref={ref} onSubmit={submit}>
        <h2 id="merge-people-title">Merge {people.length} people into one?</h2>
        <fieldset className="merge-keep" disabled={busy}>
          <legend>Keep this person</legend>
          {people.map((p) => (
            <label key={p.id} className="merge-keep-row">
              <input type="radio" name="merge-keep" value={p.id} checked={p.id === into} onChange={() => setInto(p.id)} />
              {p.cover ? (
                <FaceCrop url={p.cover.preview_url} box={p.cover.box} className="merge-keep-face" />
              ) : (
                <span className="face-crop round merge-keep-face blank" aria-hidden="true" />
              )}
              <span className="merge-keep-text">
                <span className="merge-keep-name">{personLabel(p)}</span>
                <span className="muted small">{photosWord(p.photos)}</span>
              </span>
            </label>
          ))}
        </fieldset>
        <label htmlFor="merge-people-name" className="sr-only">
          Name (optional)
        </label>
        <input
          id="merge-people-name"
          className="text-input"
          placeholder="Name (optional)"
          value={name}
          maxLength={100}
          disabled={busy}
          onChange={(e) => {
            setName(e.target.value);
            setProblem(null);
          }}
        />
        <p className="muted small">
          The photos of the others go to {target ? personLabel(target) : "the person you keep"}, and the others are deleted. Their name stays; if they have
          none, the first name among the others is used. A name typed here replaces it.
        </p>
        {(problem ?? error) && (
          <p role="alert" className="form-error">
            {problem ?? error}
          </p>
        )}
        <div className="modal-actions">
          <button type="button" className="btn btn-quiet" onClick={onCancel} disabled={busy}>
            Cancel
          </button>
          <button type="submit" className="btn btn-primary" disabled={busy || !target}>
            Merge
          </button>
        </div>
      </form>
    </div>
  );
}

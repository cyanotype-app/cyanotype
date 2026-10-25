import { useEffect, useRef, useState, type FormEvent } from "react";
import { api, ApiError, type Person, type PhotoFace } from "../api";
import { filterablePeople, nameProblem, personLabel, photosWord } from "../peopleText";

const NEW = "__new__";

/**
 * "Who is this?" for a face nobody has claimed: pick a person, or make a new one (the name is optional). Escape cancels.
 * Calls `onDone` after the server took it.
 */
export function AssignFaceDialog({ face, onDone, onCancel }: { face: PhotoFace; onDone: () => void; onCancel: () => void }) {
  const [people, setPeople] = useState<Person[] | null>(null);
  const [choice, setChoice] = useState(NEW);
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const ref = useRef<HTMLFormElement>(null);

  useEffect(() => {
    let live = true;
    api
      .listPeople()
      .then((r) => live && setPeople(filterablePeople(r.items)))
      .catch((e: unknown) => live && setError(e instanceof Error ? e.message : "Could not load your people."));
    return () => {
      live = false;
    };
  }, []);
  useEffect(() => {
    const prev = document.activeElement as HTMLElement | null;
    ref.current?.querySelector("select")?.focus();
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

  async function submit(e: FormEvent) {
    e.preventDefault();
    const problem = choice === NEW ? nameProblem(name) : null;
    if (problem) return setError(problem);
    setBusy(true);
    setError(null);
    try {
      await api.assignFace(face.id, choice === NEW ? { new: true, ...(name.trim() ? { name: name.trim() } : {}) } : { person_id: choice });
      onDone();
    } catch (err) {
      setError(err instanceof ApiError || err instanceof Error ? err.message : "Could not save.");
      setBusy(false);
    }
  }

  return (
    <div className="modal-backdrop">
      <form className="modal card" role="dialog" aria-modal="true" aria-labelledby="assign-face-title" ref={ref} onSubmit={(e) => void submit(e)}>
        <h2 id="assign-face-title">Who is this?</h2>
        <label htmlFor="assign-face-person" className="sr-only">
          Person
        </label>
        <select id="assign-face-person" className="text-input" value={choice} onChange={(e) => setChoice(e.target.value)} disabled={busy || people === null}>
          <option value={NEW}>A new person</option>
          {(people ?? []).map((p) => (
            <option key={p.id} value={p.id}>
              {personLabel(p)} ({photosWord(p.photos)})
            </option>
          ))}
        </select>
        {choice === NEW && (
          <>
            <label htmlFor="assign-face-name" className="sr-only">
              Name (optional)
            </label>
            <input
              id="assign-face-name"
              className="text-input"
              placeholder="Name (optional)"
              value={name}
              maxLength={100}
              onChange={(e) => setName(e.target.value)}
              disabled={busy}
            />
          </>
        )}
        {error && (
          <p role="alert" className="form-error">
            {error}
          </p>
        )}
        <div className="modal-actions">
          <button type="button" className="btn btn-quiet" onClick={onCancel} disabled={busy}>
            Cancel
          </button>
          <button type="submit" className="btn btn-primary" disabled={busy || people === null}>
            Save
          </button>
        </div>
      </form>
    </div>
  );
}

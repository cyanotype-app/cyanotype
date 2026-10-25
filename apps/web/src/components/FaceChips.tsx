import { useEffect, useRef, useState, type FormEvent } from "react";
import { api, ApiError, type FaceBox, type Person, type PhotoFace } from "../api";
import { filterablePeople, nameProblem, personLabel, photosWord, suggestionQuestion } from "../peopleText";
import { FaceInPhoto } from "./FaceInPhoto";

const NEW = "__new__";

/** Escape cancels a face dialog (before the viewer or the page behind it hears it), and focus goes back to where it was on close. */
function useDialogKeys(ref: { current: HTMLElement | null }, focus: string, onCancel: () => void) {
  useEffect(() => {
    const prev = document.activeElement as HTMLElement | null;
    ref.current?.querySelector<HTMLElement>(focus)?.focus();
    return () => prev?.focus?.();
    // The first focus is the dialog's own, once.
    // eslint-disable-next-line react-hooks/exhaustive-deps
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
}

/**
 * "Who is this?" for a face nobody has claimed: the photo with the face outlined (when its picture is known), then pick a person, or
 * make a new one (the name is optional), or say it is not a face at all. Escape cancels. Calls `onDone` after the server took it, and
 * `onNotAFace` (with the person the detection was in, if any) after it was marked.
 */
export function AssignFaceDialog({
  face,
  previewUrl,
  onDone,
  onNotAFace,
  onCancel,
}: {
  face: PhotoFace;
  previewUrl?: string | null;
  onDone: () => void;
  onNotAFace?: (face: PhotoFace, was: string | null) => void;
  onCancel: () => void;
}) {
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
  useDialogKeys(ref, "select", onCancel);

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

  async function notAFace() {
    setBusy(true);
    setError(null);
    try {
      const r = await api.markNotAFace(face.id);
      onNotAFace?.(face, r.person_id);
    } catch (err) {
      setError(err instanceof ApiError || err instanceof Error ? err.message : "Could not save.");
      setBusy(false);
    }
  }

  return (
    <div className="modal-backdrop">
      <form className="modal card" role="dialog" aria-modal="true" aria-labelledby="assign-face-title" ref={ref} onSubmit={(e) => void submit(e)}>
        <h2 id="assign-face-title">Who is this?</h2>
        {previewUrl && <FaceInPhoto url={previewUrl} box={face.box} />}
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
          {onNotAFace && (
            <button type="button" className="btn btn-quiet modal-actions-start" onClick={() => void notAFace()} disabled={busy}>
              Not a face
            </button>
          )}
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

/**
 * "Is this X?" for one suggested face: the photo with the face outlined, then Yes, No (not this person) or Not a face. The answers
 * are the caller's (it knows how to save them and what to tell the owner); a rejected one shows its message and keeps the dialog.
 * Escape cancels.
 */
export function ConfirmFaceDialog({
  person,
  previewUrl,
  box,
  onYes,
  onNo,
  onNotAFace,
  onCancel,
}: {
  person: { name: string | null };
  previewUrl: string;
  box: FaceBox;
  onYes: () => Promise<void>;
  onNo: () => Promise<void>;
  onNotAFace: () => Promise<void>;
  onCancel: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const ref = useRef<HTMLDivElement>(null);
  const name = personLabel(person);
  useDialogKeys(ref, "button.btn-primary", onCancel);
  async function answer(run: () => Promise<void>) {
    setBusy(true);
    setError(null);
    try {
      await run();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not save.");
      setBusy(false);
    }
  }
  return (
    <div className="modal-backdrop">
      <div className="modal card" role="dialog" aria-modal="true" aria-labelledby="confirm-face-title" ref={ref}>
        <h2 id="confirm-face-title">{suggestionQuestion(person)}</h2>
        <FaceInPhoto url={previewUrl} box={box} />
        {error && (
          <p role="alert" className="form-error">
            {error}
          </p>
        )}
        <div className="modal-actions">
          <button type="button" className="btn btn-quiet modal-actions-start" disabled={busy} onClick={() => void answer(onNotAFace)}>
            Not a face
          </button>
          <button type="button" className="btn btn-quiet" aria-label={`No, this is not ${name}`} disabled={busy} onClick={() => void answer(onNo)}>
            No
          </button>
          <button type="button" className="btn btn-primary" aria-label={`Yes, this is ${name}`} disabled={busy} onClick={() => void answer(onYes)}>
            Yes
          </button>
        </div>
      </div>
    </div>
  );
}

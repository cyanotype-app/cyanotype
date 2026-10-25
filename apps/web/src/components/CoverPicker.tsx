import { useCallback, useEffect, useRef, useState } from "react";
import { api, ApiError, type CoverFace, type Person } from "../api";
import { personLabel } from "../peopleText";
import { FaceCrop } from "./FaceCrop";

/** Faces per page of the picker. */
const PAGE = 24;

/**
 * "Change cover": the person's faces as round crops, best first, a page at a time ("Show more"). Each is a button, so the picker
 * works with the keyboard and a screen reader (the faces are numbered; the current cover says so). Choosing one calls `onChoose`
 * with the face; Escape or Cancel closes it.
 */
export function CoverPicker({
  person,
  onChoose,
  onCancel,
}: {
  person: Person;
  onChoose: (faceId: string) => Promise<void>;
  onCancel: () => void;
}) {
  const [faces, setFaces] = useState<CoverFace[] | null>(null);
  const [next, setNext] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  // After "Show more", focus moves to the first face that arrived (the button sits below them).
  const focusAt = useRef<number | null>(null);

  const load = useCallback(
    async (cursor: string | null) => {
      try {
        const r = await api.personCoverFaces(person.id, { cursor, limit: PAGE });
        setFaces((cur) => [...(cursor ? (cur ?? []) : []), ...r.items]);
        setNext(r.next_cursor);
        setError(null);
      } catch (e) {
        setError(e instanceof ApiError || e instanceof Error ? e.message : "Could not load the faces.");
      }
    },
    [person.id],
  );
  useEffect(() => {
    void load(null);
  }, [load]);
  useEffect(() => {
    if (focusAt.current === null || !faces) return;
    ref.current?.querySelectorAll<HTMLElement>(".cover-choice")[focusAt.current]?.focus();
    focusAt.current = null;
  }, [faces]);

  // Focus goes in, and back to the button that opened the picker.
  useEffect(() => {
    const prev = document.activeElement as HTMLElement | null;
    ref.current?.querySelector<HTMLElement>("button")?.focus();
    return () => prev?.focus?.();
  }, []);
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        onCancel();
      } else if (e.key === "Tab" && ref.current) {
        const f = [...ref.current.querySelectorAll<HTMLElement>("button:not(:disabled)")];
        const [first, last] = [f[0], f[f.length - 1]];
        if (e.shiftKey && document.activeElement === first) {
          e.preventDefault();
          last?.focus();
        } else if (!e.shiftKey && document.activeElement === last) {
          e.preventDefault();
          first?.focus();
        }
      }
    }
    document.addEventListener("keydown", onKey, true);
    return () => document.removeEventListener("keydown", onKey, true);
  }, [onCancel]);

  async function choose(face: CoverFace) {
    setBusy(true);
    try {
      await onChoose(face.face_id);
    } catch (e) {
      setError(e instanceof ApiError || e instanceof Error ? e.message : "Could not change the cover.");
      setBusy(false);
    }
  }

  const current = person.cover?.face_id;
  return (
    <div className="modal-backdrop">
      <div className="modal card modal-wide" role="dialog" aria-modal="true" aria-labelledby="cover-picker-title" ref={ref}>
        <h2 id="cover-picker-title">Change cover of {personLabel(person)}</h2>
        {faces === null && !error && (
          <p className="muted" role="status">
            Loading the faces…
          </p>
        )}
        {faces !== null && faces.length === 0 && <p className="muted">There are no faces to choose from yet.</p>}
        {faces !== null && faces.length > 0 && (
          <ul className="cover-grid" aria-label="Faces">
            {faces.map((f, i) => (
              <li key={f.face_id}>
                <button
                  type="button"
                  className="cover-choice"
                  aria-pressed={f.face_id === current}
                  aria-label={`Face ${i + 1}${f.face_id === current ? ", the current cover" : ""}`}
                  disabled={busy}
                  onClick={() => void choose(f)}
                >
                  <FaceCrop url={f.preview_url} box={f.box} className="cover-choice-face" />
                </button>
              </li>
            ))}
          </ul>
        )}
        {error && (
          <p role="alert" className="form-error">
            {error}{" "}
            <button type="button" className="btn btn-quiet" onClick={() => void load(faces && next ? next : null)}>
              Retry
            </button>
          </p>
        )}
        <div className="modal-actions">
          {next && (
            <button type="button" className="btn btn-quiet" disabled={busy} onClick={() => {
                focusAt.current = faces?.length ?? 0;
                void load(next);
              }}>
              Show more
            </button>
          )}
          <button type="button" className="btn btn-quiet" onClick={onCancel} disabled={busy}>
            Cancel
          </button>
        </div>
      </div>
    </div>
  );
}

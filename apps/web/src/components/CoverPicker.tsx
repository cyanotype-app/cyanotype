import { useCallback, useEffect, useRef, useState } from "react";
import { api, ApiError, type CoverFace, type Person } from "../api";
import { personLabel } from "../peopleText";
import { Button } from "../ui/Button";
import { cn } from "../ui/cn";
import { FaceCrop } from "./FaceCrop";
import { FormError, Modal } from "./sheetForm";

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
    ref.current?.querySelectorAll<HTMLElement>("[data-cover-choice]")[focusAt.current]?.focus();
    focusAt.current = null;
  }, [faces]);

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
    <Modal
      onClose={onCancel}
      title={`Change cover of ${personLabel(person)}`}
      size="lg"
      footer={
        <Button variant="secondary" onClick={onCancel} disabled={busy}>
          Cancel
        </Button>
      }
    >
      <div ref={ref} className="flex flex-col gap-4">
        {faces === null && !error && (
          <p className="text-sm text-muted" role="status">
            Loading the faces…
          </p>
        )}
        {faces !== null && faces.length === 0 && <p className="text-sm text-muted">There are no faces to choose from yet.</p>}
        {faces !== null && faces.length > 0 && (
          <ul className="grid grid-cols-4 gap-3 sm:grid-cols-6" aria-label="Faces">
            {faces.map((f, i) => (
              <li key={f.face_id}>
                <button
                  type="button"
                  className={cn(
                    "block w-full rounded-full p-0.5 transition-transform duration-150 ease-soft hover:scale-105 disabled:opacity-60",
                    f.face_id === current && "ring-2 ring-accent",
                  )}
                  data-cover-choice=""
                  aria-pressed={f.face_id === current}
                  aria-label={`Face ${i + 1}${f.face_id === current ? ", the current cover" : ""}`}
                  disabled={busy}
                  onClick={() => void choose(f)}
                >
                  <FaceCrop url={f.preview_url} box={f.box} />
                </button>
              </li>
            ))}
          </ul>
        )}
        {error && (
          <div className="flex items-center gap-2">
            <FormError>{error}</FormError>
            <Button variant="quiet" size="sm" onClick={() => void load(faces && next ? next : null)}>
              Retry
            </Button>
          </div>
        )}
        {next && (
          <Button
            variant="secondary"
            className="self-center"
            disabled={busy}
            onClick={() => {
              focusAt.current = faces?.length ?? 0;
              void load(next);
            }}
          >
            Show more
          </Button>
        )}
      </div>
    </Modal>
  );
}

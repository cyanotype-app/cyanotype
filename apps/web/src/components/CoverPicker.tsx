import { useCallback, useEffect, useRef, useState } from "react";
import { api, ApiError, type CoverFace, type FaceBox, type Person } from "../api";
import { personLabel } from "../peopleText";
import { Button } from "../ui/Button";
import { cn } from "../ui/cn";
import { FaceCrop } from "./FaceCrop";
import { FormError, Modal } from "./sheetForm";

/** Faces per page of the picker. */
const PAGE = 24;

/** One thing a cover can be: a face or an animal, in the picture `preview_url` serves. */
export interface CoverChoice {
  id: string;
  preview_url: string;
  box: FaceBox;
}

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
  return (
    <CoverChoices
      title={`Change cover of ${personLabel(person)}`}
      noun="face"
      current={person.cover?.face_id}
      load={async (cursor, limit) => {
        const r = await api.personCoverFaces(person.id, { cursor, limit });
        return { items: r.items.map((f: CoverFace) => ({ id: f.face_id, preview_url: f.preview_url, box: f.box })), next_cursor: r.next_cursor };
      }}
      loadKey={person.id}
      onChoose={onChoose}
      onCancel={onCancel}
    />
  );
}

/** The same picker, for what a cover can be chosen from (`noun`: "face" or "animal"), a page at a time from `load`. */
export function CoverChoices({
  title,
  noun,
  current,
  load: loadPage,
  loadKey,
  onChoose,
  onCancel,
}: {
  title: string;
  noun: "face" | "animal";
  current?: string;
  load: (cursor: string | null, limit: number) => Promise<{ items: CoverChoice[]; next_cursor: string | null }>;
  /** Changes when `load` asks another list (the list is read again). */
  loadKey: string;
  onChoose: (id: string) => Promise<void>;
  onCancel: () => void;
}) {
  const Noun = noun === "face" ? "Face" : "Animal";
  const plural = noun === "face" ? "faces" : "animals";
  const [faces, setFaces] = useState<CoverChoice[] | null>(null);
  const [next, setNext] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  // After "Show more", focus moves to the first face that arrived (the button sits below them).
  const focusAt = useRef<number | null>(null);

  const load = useCallback(
    async (cursor: string | null) => {
      try {
        const r = await loadPage(cursor, PAGE);
        setFaces((cur) => [...(cursor ? (cur ?? []) : []), ...r.items]);
        setNext(r.next_cursor);
        setError(null);
      } catch (e) {
        setError(e instanceof ApiError || e instanceof Error ? e.message : `Could not load the ${plural}.`);
      }
    },
    // `loadPage` is the caller's closure: the list is asked again when `loadKey` changes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [loadKey, plural],
  );
  useEffect(() => {
    void load(null);
  }, [load]);
  useEffect(() => {
    if (focusAt.current === null || !faces) return;
    ref.current?.querySelectorAll<HTMLElement>("[data-cover-choice]")[focusAt.current]?.focus();
    focusAt.current = null;
  }, [faces]);

  async function choose(face: CoverChoice) {
    setBusy(true);
    try {
      await onChoose(face.id);
    } catch (e) {
      setError(e instanceof ApiError || e instanceof Error ? e.message : "Could not change the cover.");
      setBusy(false);
    }
  }

  return (
    <Modal
      onClose={onCancel}
      title={title}
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
            Loading the {plural}…
          </p>
        )}
        {faces !== null && faces.length === 0 && <p className="text-sm text-muted">There are no {plural} to choose from yet.</p>}
        {faces !== null && faces.length > 0 && (
          <ul className="grid grid-cols-4 gap-3 sm:grid-cols-6" aria-label={Noun + "s"}>
            {faces.map((f, i) => (
              <li key={f.id}>
                <button
                  type="button"
                  className={cn(
                    "block w-full rounded-full p-0.5 transition-transform duration-150 ease-soft hover:scale-105 disabled:opacity-60",
                    f.id === current && "ring-2 ring-accent",
                  )}
                  data-cover-choice=""
                  aria-pressed={f.id === current}
                  aria-label={`${Noun} ${i + 1}${f.id === current ? ", the current cover" : ""}`}
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

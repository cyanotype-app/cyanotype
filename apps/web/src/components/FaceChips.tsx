import { useEffect, useId, useState, type FormEvent } from "react";
import { api, ApiError, type FaceBox, type Person, type PhotoFace } from "../api";
import { filterablePeople, nameProblem, personLabel, photosWord, suggestionQuestion } from "../peopleText";
import { Button } from "../ui/Button";
import { FaceInPhoto } from "./FaceInPhoto";
import { FormError, Modal, SelectBox, TextInput } from "./sheetForm";

const NEW = "__new__";

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
  const form = useId();

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
    <Modal
      onClose={onCancel}
      title="Who is this?"
      size="sm"
      footer={
        <>
          {onNotAFace && (
            <Button variant="quiet" className="md:mr-auto" onClick={() => void notAFace()} disabled={busy}>
              Not a face
            </Button>
          )}
          <Button variant="secondary" onClick={onCancel} disabled={busy}>
            Cancel
          </Button>
          <Button variant="primary" type="submit" form={form} disabled={busy || people === null}>
            Save
          </Button>
        </>
      }
    >
      <form id={form} className="flex flex-col gap-3" onSubmit={(e) => void submit(e)}>
        {previewUrl && <FaceInPhoto url={previewUrl} box={face.box} />}
        <label htmlFor={`${form}-person`} className="sr-only">
          Person
        </label>
        <SelectBox id={`${form}-person`} data-autofocus="" value={choice} onChange={(e) => setChoice(e.target.value)} disabled={busy || people === null}>
          <option value={NEW}>A new person</option>
          {(people ?? []).map((p) => (
            <option key={p.id} value={p.id}>
              {personLabel(p)} ({photosWord(p.photos)})
            </option>
          ))}
        </SelectBox>
        {choice === NEW && (
          <>
            <label htmlFor={`${form}-name`} className="sr-only">
              Name (optional)
            </label>
            <TextInput
              id={`${form}-name`}
              placeholder="Name (optional)"
              value={name}
              maxLength={100}
              onChange={(e) => setName(e.target.value)}
              disabled={busy}
            />
          </>
        )}
        {error && <FormError>{error}</FormError>}
      </form>
    </Modal>
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
  label,
}: {
  person: { name: string | null };
  previewUrl: string;
  box: FaceBox;
  onYes: () => Promise<void>;
  onNo: () => Promise<void>;
  /** Without it there is no "Not a face" (a pet's animal is refused, not dismissed). */
  onNotAFace?: () => Promise<void>;
  onCancel: () => void;
  /** What the picture is called when it is not a face that is outlined. */
  label?: string;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const name = personLabel(person);
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
    <Modal
      onClose={onCancel}
      title={suggestionQuestion(person)}
      size="sm"
      footer={
        <>
          {onNotAFace && (
            <Button variant="quiet" className="md:mr-auto" disabled={busy} onClick={() => void answer(onNotAFace)}>
              Not a face
            </Button>
          )}
          <Button variant="secondary" aria-label={`No, this is not ${name}`} disabled={busy} onClick={() => void answer(onNo)}>
            No
          </Button>
          <Button variant="primary" data-autofocus="" aria-label={`Yes, this is ${name}`} disabled={busy} onClick={() => void answer(onYes)}>
            Yes
          </Button>
        </>
      }
    >
      <FaceInPhoto url={previewUrl} box={box} label={label} />
      {error && <FormError>{error}</FormError>}
    </Modal>
  );
}

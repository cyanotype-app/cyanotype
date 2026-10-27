import { useEffect, useId, useState, type FormEvent } from "react";
import { api, ApiError, type AmbiguousPhoto, type NamedPet, type PetAdded } from "../api";
import { MAX_NAME_CHARS } from "../peopleText";
import { addedToPetMessage, filterablePets, petNameProblem, skippedLines } from "../petsText";
import { Button } from "../ui/Button";
import { Segmented } from "../ui/Controls";
import { AnimalPicker } from "./AnimalPicker";
import { Field, FormError, Modal, Note, SelectBox, TextInput } from "./sheetForm";

const NEW = "__new__";

/** What the sheet did: the pet (its name even when the server could not list it), how many photos joined, and whether it was made now. */
export interface PetNamed {
  pet: NamedPet | null;
  name: string;
  assigned: number;
  made: boolean;
}

/** The toast for what the sheet did, with a way to the pet's page when it is known. */
export function petNamedToast(r: PetNamed, open: (petId: string) => void): { message: string; action?: { label: string; onClick: () => void } } {
  const message = addedToPetMessage(r.name, r.assigned, r.made);
  return r.pet ? { message, action: { label: "Open", onClick: () => open(r.pet!.id) } } : { message };
}

type Step =
  | { name: "form" }
  | { name: "pick"; pet: NamedPet | null; kind: "dog" | "cat"; petName: string; made: boolean; assigned: number; todo: AmbiguousPhoto[]; chosen: string[]; skipped: PetAdded["skipped"] }
  | { name: "done"; pet: NamedPet | null; kind: "dog" | "cat"; petName: string; made: boolean; assigned: number; skipped: PetAdded["skipped"] };

/**
 * "Name pet…" for photos the owner picked (or "Add to pet…" for one animal of a photo, with its kind): give a new pet a name and a kind, or add
 * the photos to a pet there already. A photo with
 * several animals of the kind (the server answers `ambiguous`) is shown with each animal outlined, to tap the right one; photos that
 * could not join are listed with the reason in plain words. Calls `onDone` once everything is settled (at once when nothing was left out).
 */
export function NamePetSheet({
  assetIds,
  detectionIds,
  kind: fixedKind,
  onClose,
  onDone,
}: {
  assetIds?: readonly string[];
  /** Animals instead of photos: the owner's explicit word (an animal of another pet moves). */
  detectionIds?: readonly string[];
  /** The kind of the animals, when it is known (then it is not asked, and only pets of it are offered). */
  kind?: "dog" | "cat";
  onClose: () => void;
  onDone: (r: PetNamed) => void;
}) {
  const form = useId();
  const [pets, setPets] = useState<NamedPet[] | null>(null);
  const [choice, setChoice] = useState(NEW);
  const [name, setName] = useState("");
  const [kind, setKind] = useState<"dog" | "cat">(fixedKind ?? "dog");
  const [step, setStep] = useState<Step>({ name: "form" });
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [picked, setPicked] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    api.namedPets().then(
      (r) => live && setPets(filterablePets(r.items).filter((p) => !fixedKind || p.kind === fixedKind)),
      () => live && setPets([]),
    );
    return () => {
      live = false;
    };
  }, [fixedKind]);

  const what = { ...(assetIds ? { asset_ids: assetIds } : {}), ...(detectionIds ? { detection_ids: detectionIds } : {}) };
  const existing = choice === NEW ? null : (pets?.find((p) => p.id === choice) ?? null);
  const message = (e: unknown) => (e instanceof ApiError || e instanceof Error ? e.message : "Could not save.");

  /** Settle what the server answered: ask about the photos with several animals, or finish. */
  function settle(r: PetAdded, base: { pet: NamedPet | null; kind: "dog" | "cat"; petName: string; made: boolean; assigned: number; chosen: string[]; skipped: PetAdded["skipped"] }) {
    const pet = r.pet ?? base.pet;
    const assigned = base.assigned + r.assigned;
    const skipped = [...base.skipped, ...r.skipped];
    if (r.ambiguous.length > 0) {
      setPicked(null);
      setStep({ name: "pick", ...base, pet, assigned, skipped, todo: r.ambiguous });
      return;
    }
    finish({ ...base, pet, assigned, skipped });
  }
  function finish(r: { pet: NamedPet | null; kind: "dog" | "cat"; petName: string; made: boolean; assigned: number; skipped: PetAdded["skipped"] }) {
    if (r.skipped.length === 0) onDone({ pet: r.pet, name: r.petName, assigned: r.assigned, made: r.made });
    else setStep({ ...r, name: "done" });
  }

  async function submit(e: FormEvent) {
    e.preventDefault();
    const problem = existing ? null : petNameProblem(name);
    if (problem) return setError(problem);
    setBusy(true);
    setError(null);
    try {
      if (existing) {
        const r = await api.addToPet(existing.id, what);
        settle(r, { pet: existing, kind: existing.kind, petName: existing.name, made: false, assigned: 0, chosen: [], skipped: [] });
      } else {
        const r = await api.createPet({ name: name.trim(), kind, ...what });
        settle(r, { pet: r.pet, kind, petName: r.pet?.name ?? name.trim(), made: true, assigned: 0, chosen: [], skipped: [] });
      }
    } catch (err) {
      setError(message(err));
    } finally {
      setBusy(false);
    }
  }

  /** The animal of the photo in question was chosen (or the photo skipped): next photo, or send the choices. */
  async function next(skip: boolean) {
    if (step.name !== "pick") return;
    const chosen = skip || !picked ? step.chosen : [...step.chosen, picked];
    const todo = step.todo.slice(1);
    setPicked(null);
    if (todo.length > 0) return setStep({ ...step, chosen, todo });
    if (chosen.length === 0 || !step.pet) return finish({ ...step, skipped: step.skipped });
    setBusy(true);
    setError(null);
    try {
      const r = await api.addToPet(step.pet.id, { detection_ids: chosen });
      finish({ ...step, pet: r.pet ?? step.pet, assigned: step.assigned + r.assigned, skipped: [...step.skipped, ...r.skipped] });
    } catch (err) {
      setError(message(err));
    } finally {
      setBusy(false);
    }
  }

  if (step.name === "pick") {
    const photo = step.todo[0]!;
    return (
      <Modal
        // (The pet, and the photos with one animal, are already made: closing here ends it as it stands.)
        onClose={() => onDone({ pet: step.pet, name: step.petName, assigned: step.assigned, made: step.made })}
        title={`Which one is ${step.petName}?`}
        size="md"
        footer={
          <>
            <Button variant="quiet" className="md:mr-auto" disabled={busy} onClick={() => void next(true)}>
              Skip this photo
            </Button>
            <Button variant="primary" data-autofocus="" disabled={busy || picked === null} onClick={() => void next(false)}>
              {step.todo.length > 1 ? "Next" : "Add"}
            </Button>
          </>
        }
      >
        <div className="flex flex-col gap-3">
          <p className="text-[0.9375rem] text-muted [overflow-wrap:anywhere]">
            This photo has {photo.candidates.length} {step.kind === "dog" ? "dogs" : "cats"}. Tap the one that is {step.petName}.
            {step.todo.length > 1 && ` ${step.todo.length - 1} more to go.`}
          </p>
          <AnimalPicker key={photo.asset_id} url={photo.preview_url} candidates={photo.candidates} value={picked} onChange={setPicked} />
          {error && <FormError>{error}</FormError>}
        </div>
      </Modal>
    );
  }

  if (step.name === "done") {
    const lines = skippedLines(step.skipped, { name: step.petName, kind: step.kind });
    return (
      <Modal
        onClose={() => onDone({ pet: step.pet, name: step.petName, assigned: step.assigned, made: step.made })}
        title={step.made ? `Made ${step.petName}` : `Added to ${step.petName}`}
        size="sm"
        footer={
          <Button
            variant="primary"
            data-autofocus=""
            onClick={() => onDone({ pet: step.pet, name: step.petName, assigned: step.assigned, made: step.made })}
          >
            Done
          </Button>
        }
      >
        <div className="flex flex-col gap-3 text-[0.9375rem]">
          <p>{step.assigned === 0 ? "No photo could be added." : `${step.assigned} ${step.assigned === 1 ? "photo" : "photos"} added.`}</p>
          <ul className="flex list-disc flex-col gap-1 pl-5 text-muted">
            {lines.map((l) => (
              <li key={l}>{l}</li>
            ))}
          </ul>
        </div>
      </Modal>
    );
  }

  return (
    <Modal
      onClose={onClose}
      title={detectionIds ? "Add to pet" : "Name pet"}
      size="sm"
      footer={
        <>
          <Button variant="secondary" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button variant="primary" type="submit" form={form} disabled={busy || pets === null}>
            {existing ? "Add photos" : "Save"}
          </Button>
        </>
      }
    >
      <form id={form} className="flex flex-col gap-4" onSubmit={(e) => void submit(e)}>
        <Note>
          {detectionIds
            ? "This animal becomes the pet."
            : (assetIds?.length ?? 0) === 1
              ? "The dog or cat in this photo becomes the pet."
              : `The dog or cat in each of these ${assetIds?.length ?? 0} photos becomes the pet.`}{" "}
          A few photos are enough: more are suggested for you to confirm.
        </Note>
        {pets !== null && pets.length > 0 && (
          <Field label="Pet" htmlFor={`${form}-pet`}>
            <SelectBox id={`${form}-pet`} value={choice} onChange={(e) => setChoice(e.target.value)} disabled={busy}>
              <option value={NEW}>A new pet</option>
              {pets.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.name} ({p.kind})
                </option>
              ))}
            </SelectBox>
          </Field>
        )}
        {!existing && (
          <>
            <Field label="Name" htmlFor={`${form}-name`}>
              <TextInput
                id={`${form}-name`}
                data-autofocus=""
                placeholder="Name"
                value={name}
                maxLength={MAX_NAME_CHARS}
                autoComplete="off"
                onChange={(e) => {
                  setName(e.target.value);
                  setError(null);
                }}
                aria-invalid={error && petNameProblem(name) ? true : undefined}
                disabled={busy}
              />
            </Field>
            {!fixedKind && (
              <Field label="Kind">
                <Segmented
                  label="Kind"
                  value={kind}
                  onChange={setKind}
                  options={[
                    { value: "dog", label: "Dog" },
                    { value: "cat", label: "Cat" },
                  ]}
                />
              </Field>
            )}
          </>
        )}
        {error && <FormError>{error}</FormError>}
      </form>
    </Modal>
  );
}

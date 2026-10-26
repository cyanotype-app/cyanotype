import { useId, useState, type FormEvent } from "react";
import type { Person } from "../api";
import { isNamed, nameProblem, personLabel, photosWord } from "../peopleText";
import { Button } from "../ui/Button";
import { BlankFace, FaceCrop } from "./FaceCrop";
import { FormError, Modal, Note, TextInput } from "./sheetForm";

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
  const id = useId();
  const [into, setInto] = useState(() => defaultSurvivor(people)?.id ?? "");
  const [name, setName] = useState("");
  const [problem, setProblem] = useState<string | null>(null);

  const target = people.find((p) => p.id === into);
  function submit(e: FormEvent) {
    e.preventDefault();
    if (!target) return;
    const bad = nameProblem(name);
    if (bad) return setProblem(bad);
    onMerge(target, name.trim() === "" ? undefined : name.trim());
  }
  return (
    <Modal
      onClose={onCancel}
      title={`Merge ${people.length} people into one?`}
      footer={
        <>
          <Button variant="secondary" onClick={onCancel} disabled={busy}>
            Cancel
          </Button>
          <Button variant="primary" type="submit" form={id} disabled={busy || !target}>
            Merge
          </Button>
        </>
      }
    >
      <form id={id} className="flex flex-col gap-4" onSubmit={submit}>
        <fieldset className="m-0 min-w-0 border-0 p-0" disabled={busy}>
          <legend className="mb-1.5 p-0 text-[0.8125rem] font-medium text-muted">Keep this person</legend>
          <div className="flex flex-col">
            {people.map((p) => (
              <label
                key={p.id}
                className="flex min-h-14 cursor-pointer items-center gap-3 rounded-card px-1.5 hover:bg-surface has-[:checked]:bg-surface has-[:disabled]:cursor-default"
              >
                <input
                  type="radio"
                  name={`${id}-keep`}
                  value={p.id}
                  className="size-5 shrink-0 accent-accent"
                  checked={p.id === into}
                  data-autofocus={p.id === into ? "" : undefined}
                  onChange={() => setInto(p.id)}
                />
                {p.cover ? <FaceCrop url={p.cover.preview_url} box={p.cover.box} className="size-11 shrink-0" /> : <BlankFace className="size-11 shrink-0" />}
                <span className="min-w-0">
                  <span className="block truncate text-[0.9375rem]">{personLabel(p)}</span>
                  <span className="block text-[0.8125rem] text-muted">{photosWord(p.photos)}</span>
                </span>
              </label>
            ))}
          </div>
        </fieldset>
        <label htmlFor={`${id}-name`} className="sr-only">
          Name (optional)
        </label>
        <TextInput
          id={`${id}-name`}
          placeholder="Name (optional)"
          value={name}
          maxLength={100}
          disabled={busy}
          onChange={(e) => {
            setName(e.target.value);
            setProblem(null);
          }}
        />
        <Note>
          The photos of the others go to {target ? personLabel(target) : "the person you keep"}, and the others are deleted. Their name stays; if they have
          none, the first name among the others is used. A name typed here replaces it.
        </Note>
        {(problem ?? error) && <FormError>{problem ?? error}</FormError>}
      </form>
    </Modal>
  );
}

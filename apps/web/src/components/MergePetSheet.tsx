import { useEffect, useId, useState } from "react";
import { api, type NamedPet } from "../api";
import { FIND_A_PET, petPhotos } from "../petsText";
import { matchingPeople } from "../peopleText";
import { Button } from "../ui/Button";
import { BlankFace, FaceCrop } from "./FaceCrop";
import { FormError, inputClass, Modal, Note } from "./sheetForm";

/** Choose the pet this one becomes part of: only pets of the same kind (the server refuses another), the others are not offered. */
export function MergePetSheet({
  from,
  busy,
  error,
  onMerge,
  onCancel,
}: {
  from: NamedPet;
  busy: boolean;
  error: string | null;
  onMerge: (into: NamedPet) => void;
  onCancel: () => void;
}) {
  const id = useId();
  const [others, setOthers] = useState<NamedPet[] | null>(null);
  const [into, setInto] = useState("");
  const [typed, setTyped] = useState("");
  const [loadError, setLoadError] = useState<string | null>(null);
  useEffect(() => {
    let live = true;
    api
      .namedPets()
      .then((r) => {
        if (!live) return;
        const list = r.items.filter((p) => p.id !== from.id && p.kind === from.kind);
        setOthers(list);
        setInto(list[0]?.id ?? "");
      })
      .catch((e: unknown) => live && setLoadError(e instanceof Error ? e.message : "Could not load your pets."));
    return () => {
      live = false;
    };
  }, [from.id, from.kind]);
  const target = others?.find((p) => p.id === into);
  const shown = matchingPeople(others ?? [], typed, [into]);
  const kind = from.kind === "dog" ? "dog" : "cat";
  return (
    <Modal
      onClose={onCancel}
      title={`Merge ${from.name} into…`}
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
      <form
        id={id}
        className="flex flex-col gap-3"
        onSubmit={(e) => {
          e.preventDefault();
          if (target) onMerge(target);
        }}
      >
        {others !== null && others.length > 6 && (
          <>
            <label htmlFor={`${id}-find`} className="sr-only">
              {FIND_A_PET}
            </label>
            <input id={`${id}-find`} className={inputClass} type="search" placeholder={FIND_A_PET} value={typed} maxLength={100} onChange={(e) => setTyped(e.target.value)} />
          </>
        )}
        <fieldset className="m-0 min-w-0 border-0 p-0" disabled={busy || !others?.length}>
          <legend className="sr-only">Merge into</legend>
          <div className="flex max-h-72 flex-col overflow-y-auto">
            {shown.map((p) => (
              <label key={p.id} className="flex min-h-14 cursor-pointer items-center gap-3 rounded-card px-1.5 hover:bg-surface has-[:checked]:bg-surface has-[:disabled]:cursor-default">
                <input
                  type="radio"
                  name={`${id}-into`}
                  className="size-5 shrink-0 accent-accent"
                  checked={p.id === into}
                  data-autofocus={p.id === into ? "" : undefined}
                  onChange={() => setInto(p.id)}
                />
                {p.cover ? <FaceCrop url={p.cover.preview_url} box={p.cover.box} className="size-11 shrink-0" /> : <BlankFace className="size-11 shrink-0" />}
                <span className="min-w-0">
                  <span className="block truncate text-[0.9375rem]">{p.name}</span>
                  <span className="block text-[0.8125rem] text-muted">{petPhotos(p.photos)}</span>
                </span>
              </label>
            ))}
          </div>
        </fieldset>
        {others !== null && others.length === 0 && <Note>{`You have no other ${kind} to merge into. Only pets of the same kind can be merged.`}</Note>}
        <Note>
          All of {from.name}’s photos go to the other pet, and {from.name} is deleted. The other pet keeps its name.
        </Note>
        {(error ?? loadError) && <FormError>{error ?? loadError}</FormError>}
      </form>
    </Modal>
  );
}

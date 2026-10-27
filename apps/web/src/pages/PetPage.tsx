import { Eye, EyeOff, FolderPlus, Merge, MoreHorizontal, Pencil, PawPrint, Search, Trash2 } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { api, ApiError, type NamedPet, type Scope, type Target } from "../api";
import { useAuth } from "../auth";
import { AddToAlbumDialog, addedMessage } from "../components/AddToAlbumDialog";
import { ConfirmSheet } from "../components/ConfirmSheet";
import { CoverChoices } from "../components/CoverPicker";
import { BlankFace, FaceCrop } from "../components/FaceCrop";
import { MergePetSheet } from "../components/MergePetSheet";
import { Enter, Loading, PageProblem, useTitleCut } from "../components/pageKit";
import { PetSuggestions } from "../components/PetSuggestions";
import { RenameSheet } from "../components/RenameSheet";
import { ScopePhotos, type Membership } from "../components/ScopePhotos";
import { FormError } from "../components/sheetForm";
import { useToast } from "../components/Toast";
import { pendingLabel } from "../peopleText";
import { KIND_TITLE, mergedPetMessage, petNameProblem, petPhotos, removedFromPetMessage } from "../petsText";
import { PageHeader } from "../shell/PageHeader";
import { Button } from "../ui/Button";
import { Menu, MenuItem, MenuSeparator } from "../ui/Menu";
import { usePetsEnabled } from "../usePets";
import { NotFound } from "./NotFound";

/** How often the pet's counts are read again, to notice photos added meanwhile (and the proposals that wait). */
const LIVE_EVERY_MS = 15000;

export function PetPage() {
  const { id = "" } = useParams();
  const enabled = usePetsEnabled();
  if (enabled === null) {
    return (
      <>
        <PageHeader title="Pet" back={{ to: "/pets", label: "Pets" }} />
        <Loading>Loading…</Loading>
      </>
    );
  }
  return enabled ? <PetView key={id} id={id} /> : <NotFound />;
}

/**
 * One named pet: its photos as a timeline, "Is this X?" proposals to confirm or refuse, rename, hide, cover, merge into another pet of
 * its kind, delete (the photos stay), taking photos out ("Not this pet"), and saving its photos as an album.
 */
function PetView({ id }: { id: string }) {
  const { setAnonymous } = useAuth();
  const navigate = useNavigate();
  const { toast, show } = useToast();
  const [pet, setPet] = useState<NamedPet | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [missing, setMissing] = useState(false);
  const [renaming, setRenaming] = useState(false);
  const [dialog, setDialog] = useState<"merge" | "delete" | "cover" | null>(null);
  const [dialogError, setDialogError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [saving, setSaving] = useState<{ target: Target; count: number } | null>(null);
  const nameCut = useTitleCut(pet?.name ?? "");

  const failure = useCallback(
    (e: unknown, what: string) => {
      if (e instanceof ApiError && e.status === 401) setAnonymous();
      else show({ tone: "error", message: `${what} ${e instanceof Error ? e.message : ""}`.trim() });
    },
    [setAnonymous, show],
  );

  const load = useCallback(async () => {
    try {
      setPet(await api.namedPet(id));
    } catch (e) {
      if (e instanceof ApiError && e.status === 401) setAnonymous();
      else if (e instanceof ApiError && e.status === 404) setMissing(true);
      else setProblem(e instanceof Error ? e.message : "Could not load the pet.");
    }
  }, [id, setAnonymous]);
  useEffect(() => {
    void load();
  }, [load]);
  // Live: photos can be added (or proposals found) in the background; read the counts again now and then while the page is open.
  useEffect(() => {
    const t = setInterval(() => {
      if (document.visibilityState === "hidden") return;
      api.namedPet(id).then(
        (p) =>
          setPet((cur) =>
            cur && cur.photos === p.photos && cur.detections === p.detections && cur.pending === p.pending && cur.name === p.name && cur.hidden === p.hidden ? cur : p,
          ),
        () => undefined,
      );
    }, LIVE_EVERY_MS);
    return () => clearInterval(t);
  }, [id]);

  const photosId = pet?.id;
  const view = useMemo(() => ({ pet: photosId }), [photosId]);
  const scope = useMemo<Scope>(() => ({ pet: id }), [id]);
  const membership = useMemo<Membership | undefined>(
    () =>
      pet
        ? {
            label: "Not this pet",
            title: `Not ${pet.name}?`,
            body: (photos) => (
              <p>
                {pet.name} is taken out of {petPhotos(photos)}. It will not be put back, or suggested, for this pet. The photos stay in your library.
              </p>
            ),
            remove: async (ids) => (await api.removeFromPet(id, { asset_ids: ids })).removed,
            removed: (photos) => removedFromPetMessage(photos, pet.name),
            cover: async (asset) => {
              const found = await api.assetPets(asset);
              const mine = found.items.find((a) => a.pet?.id === id);
              if (!mine) throw new Error("That photo has no animal of this pet.");
              setPet(await api.updatePet(id, { cover_detection_id: mine.id }));
            },
          }
        : undefined,
    [pet, id],
  );

  if (problem) {
    return (
      <PageProblem
        title="Couldn’t load the pet"
        message={problem}
        back={{ to: "/pets", label: "Pets" }}
        action={
          <Button asChild variant="primary">
            <Link to="/pets">All pets</Link>
          </Button>
        }
      />
    );
  }
  if (missing) {
    return (
      <PageProblem
        title="Pet not found"
        message="It may have been merged into another pet or deleted."
        back={{ to: "/pets", label: "Pets" }}
        action={
          <Button asChild variant="primary">
            <Link to="/pets">All pets</Link>
          </Button>
        }
      />
    );
  }
  if (!pet) {
    return (
      <>
        <PageHeader title="Pet" back={{ to: "/pets", label: "Pets" }} />
        <Loading>Loading the pet…</Loading>
      </>
    );
  }

  async function saveName(name: string) {
    try {
      setPet(await api.updatePet(id, { name: name.trim() }));
      setRenaming(false);
    } catch (e) {
      failure(e, "Couldn’t save the name.");
    }
  }
  async function toggleHidden() {
    if (!pet) return;
    try {
      setPet(await api.updatePet(id, { hidden: !pet.hidden }));
      show({ message: pet.hidden ? `${pet.name} is shown again` : `${pet.name} is hidden: no suggestions, and not named in search or on photos` });
    } catch (e) {
      failure(e, "Couldn’t change that.");
    }
  }
  async function chooseCover(detectionId: string) {
    setPet(await api.updatePet(id, { cover_detection_id: detectionId }));
    setDialog(null);
    show({ message: "Cover changed" });
  }
  async function merge(into: NamedPet) {
    setBusy(true);
    setDialogError(null);
    try {
      await api.mergePet(id, into.id);
      show({ message: mergedPetMessage(into.name) });
      navigate(`/pets/${into.id}`, { replace: true });
    } catch (e) {
      if (e instanceof ApiError && e.status === 401) setAnonymous();
      else setDialogError(e instanceof Error ? e.message : "Could not merge.");
      setBusy(false);
    }
  }
  async function remove() {
    setBusy(true);
    try {
      await api.deletePet(id);
      navigate("/pets", { replace: true });
    } catch (e) {
      if (e instanceof ApiError && e.status === 401) setAnonymous();
      else setDialogError(e instanceof Error ? e.message : "Could not delete the pet.");
      setBusy(false);
    }
  }

  return (
    <>
      <PageHeader
        title={pet.name}
        back={{ to: "/pets", label: "Pets" }}
        actions={
          <Menu
            label="Pet"
            trigger={
              <Button size="icon" variant="ghost" aria-label="More">
                <MoreHorizontal />
              </Button>
            }
          >
            <MenuItem onSelect={() => setRenaming(true)}>
              <Pencil />
              Rename
            </MenuItem>
            <MenuItem disabled={pet.detections === 0} onSelect={() => setDialog("cover")}>
              <PawPrint />
              Change cover
            </MenuItem>
            <MenuItem onSelect={() => void toggleHidden()}>
              {pet.hidden ? <Eye /> : <EyeOff />}
              {pet.hidden ? "Show" : "Hide"}
            </MenuItem>
            <MenuItem disabled={pet.photos === 0} onSelect={() => setSaving({ target: { scope: { pet: id }, except: [] }, count: pet.photos })}>
              <FolderPlus />
              Save as album
            </MenuItem>
            <MenuItem asChild>
              <Link to={`/search?pet=${encodeURIComponent(id)}`}>
                <Search />
                Search with filters
              </Link>
            </MenuItem>
            <MenuItem onSelect={() => setDialog("merge")}>
              <Merge />
              Merge into…
            </MenuItem>
            <MenuSeparator />
            <MenuItem danger onSelect={() => setDialog("delete")}>
              <Trash2 />
              Delete pet
            </MenuItem>
          </Menu>
        }
      />
      <Enter className="mx-auto flex max-w-[96rem] items-center gap-4 px-4 pt-2 pb-3 md:gap-6 md:px-8">
        <button
          type="button"
          className="size-24 shrink-0 rounded-full outline-offset-2 transition-transform duration-150 ease-soft hover:scale-[1.03] active:scale-[0.97] disabled:pointer-events-none md:size-28"
          aria-label="Change cover"
          disabled={pet.detections === 0}
          onClick={() => setDialog("cover")}
        >
          {pet.cover ? <FaceCrop url={pet.cover.preview_url} box={pet.cover.box} /> : <BlankFace />}
        </button>
        <div className="flex min-w-0 flex-col items-start gap-1.5">
          {nameCut && <p className="text-[1.25rem] leading-snug font-semibold tracking-tight text-pretty [overflow-wrap:anywhere]">{pet.name}</p>}
          <p className="text-[1.0625rem] font-semibold tracking-tight">{petPhotos(pet.photos)}</p>
          <p className="text-sm text-muted">{KIND_TITLE[pet.kind]}</p>
          {pet.pending > 0 && <p className="text-sm text-accent">{pendingLabel(pet.pending)}</p>}
          {pet.hidden && <p className="text-sm text-muted">Hidden: no suggestions, and not named in search or on photos</p>}
        </div>
      </Enter>
      <PetSuggestions
        key={id}
        pet={pet}
        onAnswered={() => void load()}
        failure={failure}
        show={show}
      />
      <ScopePhotos
        view={view}
        scope={scope}
        changed={`${pet.photos}/${pet.detections}`}
        words={petPhotos}
        slideshowTitle={pet.name}
        empty={`Photos where ${pet.name} is found appear here.`}
        hiddenWhere="Hidden"
        membership={membership}
        failure={failure}
        show={show}
        onChanged={load}
        onSave={(target, count) => setSaving({ target, count })}
      />
      {renaming && <RenameSheet initial={pet.name} title="Rename" problem={petNameProblem} onSave={(n) => void saveName(n)} onCancel={() => setRenaming(false)} />}
      {saving !== null && (
        <AddToAlbumDialog
          mode="save"
          ids={saving.target}
          count={saving.count}
          onClose={() => setSaving(null)}
          onDone={(r) => {
            const requested = saving.count;
            setSaving(null);
            show({ message: addedMessage(r, requested) });
          }}
        />
      )}
      {dialog === "cover" && (
        <CoverChoices
          title={`Change cover of ${pet.name}`}
          noun="animal"
          current={pet.cover?.detection_id}
          load={async (cursor, limit) => {
            const r = await api.petDetections(id, { cursor, limit });
            return { items: r.items.map((d) => ({ id: d.detection_id, preview_url: d.preview_url, box: d.box })), next_cursor: r.next_cursor };
          }}
          loadKey={id}
          onChoose={chooseCover}
          onCancel={() => setDialog(null)}
        />
      )}
      {dialog === "merge" && <MergePetSheet from={pet} busy={busy} error={dialogError} onMerge={(p) => void merge(p)} onCancel={() => setDialog(null)} />}
      {dialog === "delete" && (
        <ConfirmSheet title={`Delete ${pet.name}?`} confirmLabel="Delete pet" danger busy={busy} onCancel={() => setDialog(null)} onConfirm={() => void remove()}>
          <p>The pet and its name go. The photos stay in your library, and its animals are free again; you can name the pet again later. To keep it out of sight, hide it instead.</p>
          {dialogError && <FormError>{dialogError}</FormError>}
        </ConfirmSheet>
      )}
      {toast}
    </>
  );
}

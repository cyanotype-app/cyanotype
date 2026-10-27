import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import { useCallback, useEffect, useRef, useState } from "react";
import { api, type NamedPet, type PetSuggestion } from "../api";
import { petPhotos, petQuestion } from "../petsText";
import { Button } from "../ui/Button";
import { Segmented } from "../ui/Controls";
import { ConfirmFaceDialog } from "./FaceChips";
import { ConfirmSheet } from "./ConfirmSheet";
import { FaceCrop } from "./FaceCrop";

/** Most animals read for a tier at a time (the server's limit); answering the lot loads the next ones. */
const PAGE = 60;

type Tier = "likely" | "maybe";
type Show = (t: { message: string; tone?: "error"; action?: { label: string; onClick: () => void } }) => void;

/**
 * "Is this X?" for a named pet: animals nobody has claimed that look like it, the likely ones first and then the maybe ones, each with Yes and
 * No, or all of a tier at once. Answers are optimistic: the tile goes at once and comes back with a message if the server refuses. A Yes moves
 * the pet's centre, so the proposals are read again after one. Tap an animal to see the whole photo.
 */
export function PetSuggestions({
  pet,
  onAnswered,
  failure,
  show,
}: {
  pet: NamedPet;
  /** Something was answered: how many animals joined the pet (the page counts again either way). */
  onAnswered: (confirmed: number) => void;
  failure: (e: unknown, what: string) => void;
  show: Show;
}) {
  const reduce = useReducedMotion();
  const [counts, setCounts] = useState<{ likely: number; maybe: number } | null>(null);
  const [tier, setTier] = useState<Tier | null>(null);
  const [items, setItems] = useState<PetSuggestion[]>([]);
  const [looking, setLooking] = useState<PetSuggestion | null>(null);
  const [bulk, setBulk] = useState<"yes" | "no" | null>(null);
  const [busy, setBusy] = useState(false);
  const [again, setAgain] = useState(0);
  const chosen = useRef<Tier | null>(null);

  // Read the proposals: the counts of both tiers, and the animals of the tier on show (the likely ones, until the owner picks the other).
  useEffect(() => {
    let live = true;
    const want: Tier = chosen.current ?? "likely";
    api.petSuggestions(pet.id, { tier: want, limit: PAGE }).then(
      async (r) => {
        if (!live) return;
        // Nothing likely left: show the maybe ones instead (unless the owner chose a tier).
        if (r.items.length === 0 && want === "likely" && chosen.current === null && r.counts.maybe > 0) {
          const m = await api.petSuggestions(pet.id, { tier: "maybe", limit: PAGE });
          if (!live) return;
          setCounts(m.counts);
          setTier("maybe");
          setItems(m.items);
          return;
        }
        setCounts(r.counts);
        setTier(want);
        setItems(r.items);
      },
      () => undefined,
    );
    return () => {
      live = false;
    };
  }, [pet.id, again]);

  const settled = useCallback(
    (confirmed: number) => {
      onAnswered(confirmed);
      // A Yes moves the pet's centre, and a tier that has run dry shows the next one: read the proposals again.
      setAgain((n) => n + 1);
    },
    [onAnswered],
  );

  /** Answer some animals at once; the tiles go first and return if the server says no. */
  async function answer(list: PetSuggestion[], yes: boolean) {
    const ids = list.map((s) => s.detection_id);
    const gone = new Set(ids);
    const before = items;
    const countsBefore = counts;
    setItems((cur) => cur.filter((x) => !gone.has(x.detection_id)));
    if (tier && counts) setCounts({ ...counts, [tier]: Math.max(0, counts[tier] - ids.length) });
    try {
      await api.answerPetSuggestions(pet.id, yes ? { confirm: ids } : { reject: ids });
      if (yes) show({ message: ids.length === 1 ? `Added to ${pet.name}` : `Added ${petPhotos(ids.length)} to ${pet.name}` });
      // Run dry (or a Yes, which changes who looks like the pet): read again. Single No answers leave the rest as they are.
      if (yes || items.length - ids.length < 6) settled(yes ? ids.length : 0);
      else onAnswered(0);
    } catch (e) {
      setItems(before);
      setCounts(countsBefore);
      failure(e, "Couldn’t save your answer.");
    }
  }

  async function bulkAnswer(yes: boolean) {
    setBusy(true);
    try {
      await answer(items, yes);
    } finally {
      setBusy(false);
      setBulk(null);
    }
  }

  const total = counts ? counts.likely + counts.maybe : 0;
  if (!counts || total === 0 || tier === null) return null;
  return (
    <section className="mx-auto max-w-[96rem] px-4 pt-3 pb-2 md:px-8" aria-label="Suggestions">
      <div className="mb-3 flex flex-wrap items-center gap-x-4 gap-y-2">
        <h2 className="mr-auto min-w-0 text-[1.0625rem] font-semibold tracking-tight [overflow-wrap:anywhere]">{petQuestion(pet)}</h2>
        {counts.likely > 0 && counts.maybe > 0 && (
          <Segmented
            label="Show"
            value={tier}
            onChange={(t) => {
              chosen.current = t;
              setTier(t);
              setAgain((n) => n + 1);
            }}
            options={[
              { value: "likely", label: `Likely (${counts.likely})` },
              { value: "maybe", label: `Maybe (${counts.maybe})` },
            ]}
          />
        )}
      </div>
      <p className="mb-3 text-sm text-muted [overflow-wrap:anywhere]">
        {tier === "likely" ? "These look a lot like" : "These might be"} {pet.name}. {total === 1 ? "1 is waiting for your answer." : `${total} are waiting for your answer.`}
      </p>
      {items.length > 1 && (
        <div className="mb-3 flex flex-wrap gap-2">
          <Button variant="primary" size="sm" disabled={busy} onClick={() => setBulk("yes")}>
            Yes to these {items.length}
          </Button>
          <Button variant="secondary" size="sm" disabled={busy} onClick={() => setBulk("no")}>
            No to these {items.length}
          </Button>
        </div>
      )}
      <ul className="grid grid-cols-2 gap-3 sm:grid-cols-4 md:grid-cols-[repeat(auto-fill,minmax(9rem,1fr))]">
        <AnimatePresence initial={false}>
          {items.map((s) => (
            <motion.li
              key={s.detection_id}
              layout={!reduce}
              initial={reduce ? false : { opacity: 0, scale: 0.96 }}
              animate={{ opacity: 1, scale: 1 }}
              exit={reduce ? { opacity: 0 } : { opacity: 0, scale: 0.92 }}
              transition={{ duration: reduce ? 0 : 0.2, ease: [0.2, 0.8, 0.2, 1] }}
              className="flex flex-col gap-2 rounded-card bg-surface p-2"
            >
              <button
                type="button"
                className="block overflow-hidden rounded-[0.6rem] transition-transform duration-150 ease-soft active:scale-[0.98]"
                aria-label="See the whole photo"
                onClick={() => setLooking(s)}
              >
                <FaceCrop url={s.preview_url} box={s.box} round={false} />
              </button>
              <span className="grid grid-cols-2 gap-1.5">
                <Button variant="primary" size="sm" aria-label={`Yes, this is ${pet.name}`} onClick={() => void answer([s], true)}>
                  Yes
                </Button>
                <Button variant="secondary" size="sm" className="bg-card" aria-label={`No, this is not ${pet.name}`} onClick={() => void answer([s], false)}>
                  No
                </Button>
              </span>
            </motion.li>
          ))}
        </AnimatePresence>
      </ul>
      {looking && (
        <ConfirmFaceDialog
          person={pet}
          previewUrl={looking.preview_url}
          box={looking.box}
          label="The photo, with the animal in question outlined"
          onCancel={() => setLooking(null)}
          onYes={async () => {
            const s = looking;
            setLooking(null);
            await answer([s], true);
          }}
          onNo={async () => {
            const s = looking;
            setLooking(null);
            await answer([s], false);
          }}
        />
      )}
      {bulk && (
        <ConfirmSheet
          title={bulk === "yes" ? `Add ${items.length} to ${pet.name}?` : `None of these ${items.length} are ${pet.name}?`}
          confirmLabel={bulk === "yes" ? `Yes to ${items.length}` : `No to ${items.length}`}
          danger={bulk === "no"}
          busy={busy}
          onCancel={() => setBulk(null)}
          onConfirm={() => void bulkAnswer(bulk === "yes")}
        >
          <p>
            {bulk === "yes"
              ? `${items.length === 1 ? "This animal joins" : `These ${items.length} animals join`} ${pet.name}. You can take photos out again from ${pet.name}’s page.`
              : `They will not be suggested for ${pet.name} again.`}
          </p>
        </ConfirmSheet>
      )}
    </section>
  );
}

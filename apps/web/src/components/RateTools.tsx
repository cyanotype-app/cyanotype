import { useState } from "react";
import { api, type Target } from "../api";
import { ratedMessage, starsLabel } from "../ratingText";
import { Button } from "../ui/Button";
import { cn } from "../ui/cn";
import { Sheet } from "../ui/Sheet";
import { StarIcon } from "./Stars";

interface Props {
  /** Which photos: the selection (ids, or a whole view). */
  target: Target;
  count: number;
  disabled?: boolean;
  /** The server rated them (`changed` is how many actually changed); the page refreshes and tells the owner. */
  onDone: (rating: number, changed: number, message: string) => void;
  /** The request failed (`error` is what went wrong, for the page's own handling of a signed-out session). */
  onError: (error: unknown) => void;
}

/** "Rate" for the selection bar: a small sheet with 0 to 5 stars. */
export function RateTools({ target, count, disabled, onDone, onError }: Props) {
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);

  async function rate(rating: number) {
    setBusy(true);
    try {
      const r = await api.rateAssets(target, rating);
      setOpen(false);
      onDone(rating, r.count, ratedMessage(count, rating));
    } catch (e) {
      setOpen(false);
      onError(e);
    } finally {
      setBusy(false);
    }
  }

  return (
    <>
      <Button variant="quiet" size="sm" disabled={disabled} onClick={() => setOpen(true)}>
        Rate
      </Button>
      {open && (
        <Sheet
          open
          onOpenChange={(o) => !o && setOpen(false)}
          title="Rate photos"
          description={`${count.toLocaleString("en-US")} ${count === 1 ? "photo" : "photos"}`}
          size="sm"
        >
          <div className="flex flex-col gap-0.5 pb-2">
            {[5, 4, 3, 2, 1, 0].map((n) => (
              <button
                key={n}
                type="button"
                disabled={busy}
                onClick={() => void rate(n)}
                autoFocus={n === 5}
                className="flex min-h-12 items-center gap-3 rounded-card px-3 text-left text-[0.9375rem] hover:bg-surface disabled:opacity-45"
              >
                <span className="inline-flex gap-0.5 text-muted" aria-hidden="true">
                  {Array.from({ length: 5 }, (_, i) => (
                    <span key={i} className={cn("size-5", i < n && "text-(--star)")}>
                      <StarIcon filled={i < n} />
                    </span>
                  ))}
                </span>
                <span>{n === 0 ? "Clear rating" : starsLabel(n)}</span>
              </button>
            ))}
          </div>
        </Sheet>
      )}
    </>
  );
}

import { useState } from "react";
import { api, type Target } from "../api";
import { ratedMessage, starsLabel } from "../ratingText";
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

/** "Rate" for the selection bar: a small dialog with 0 to 5 stars. */
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
      <button type="button" className="btn btn-quiet" disabled={disabled} onClick={() => setOpen(true)}>
        Rate
      </button>
      {open && (
        <div className="modal-backdrop" onMouseDown={(e) => e.target === e.currentTarget && setOpen(false)}>
          <div
            className="modal card"
            role="dialog"
            aria-modal="true"
            aria-label="Rate photos"
            onKeyDown={(e) => {
              if (e.key === "Escape") {
                e.stopPropagation();
                setOpen(false);
              }
            }}
          >
            <h2>
              Rate {count.toLocaleString("en-US")} {count === 1 ? "photo" : "photos"}
            </h2>
            <div className="rate-menu">
              {[5, 4, 3, 2, 1, 0].map((n) => (
                <button
                  key={n}
                  type="button"
                  className="btn btn-quiet rate-option"
                  disabled={busy}
                  onClick={() => void rate(n)}
                  autoFocus={n === 5}
                >
                  <span className="stars stars-readonly" aria-hidden="true">
                    {Array.from({ length: 5 }, (_, i) => (
                      <span key={i} className={`star${i < n ? " on" : ""}`}>
                        <StarIcon filled={i < n} />
                      </span>
                    ))}
                  </span>
                  <span>{n === 0 ? "Clear rating" : starsLabel(n)}</span>
                </button>
              ))}
            </div>
            <div className="modal-actions">
              <button type="button" className="btn btn-quiet" onClick={() => setOpen(false)}>
                Cancel
              </button>
            </div>
          </div>
        </div>
      )}
    </>
  );
}

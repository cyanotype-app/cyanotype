import { cn } from "../ui/cn";
import { afterTap, clampStars, MAX_STARS, starsLabel } from "../ratingText";

/** One star, filled or outlined. */
export function StarIcon({ filled }: { filled: boolean }) {
  return (
    <svg viewBox="0 0 24 24" width="100%" height="100%" aria-hidden="true" focusable="false">
      <path
        d="m12 3.2 2.7 5.6 6.1.8-4.5 4.2 1.1 6.1L12 17l-5.4 2.9 1.1-6.1L3.2 9.6l6.1-.8z"
        fill={filled ? "currentColor" : "none"}
        stroke="currentColor"
        strokeWidth="1.8"
        strokeLinejoin="round"
      />
    </svg>
  );
}

interface Props {
  /** 0 to 5; absent is not rated. */
  value: number | undefined;
  /** Choose a rating. Tapping the star the photo already has clears it. Omit for a read-only row. */
  onChange?: (rating: number) => void;
  /** Names the group for assistive technology ("Rating of IMG_1001.jpg"). */
  label?: string;
  disabled?: boolean;
}

/** Five stars as a row of buttons (44 px targets on a phone). Keys 0 to 5 do the same in the viewer. */
export function RatingStars({ value, onChange, label = "Rating", disabled = false }: Props) {
  const v = clampStars(value);
  if (!onChange) {
    return (
      <span className="inline-flex items-center gap-0.5 text-muted" role="img" aria-label={starsLabel(v)}>
        {Array.from({ length: MAX_STARS }, (_, i) => (
          <span key={i} className={cn("size-[1.2rem]", i < v && "text-(--star)")}>
            <StarIcon filled={i < v} />
          </span>
        ))}
      </span>
    );
  }
  return (
    <span className="inline-flex items-center gap-0.5 text-muted" role="group" aria-label={`${label}: ${starsLabel(v)}`}>
      {Array.from({ length: MAX_STARS }, (_, i) => {
        const n = i + 1;
        return (
          <button
            key={n}
            type="button"
            className={cn("inline-flex size-11 items-center justify-center rounded-lg p-2.5 transition-colors hover:bg-(--star)/15 disabled:opacity-50 md:size-9 md:p-2", n <= v && "text-(--star)")}
            aria-pressed={n === v}
            aria-label={n === v ? `${starsLabel(n)}, selected. Press to clear` : starsLabel(n)}
            disabled={disabled}
            onClick={() => onChange(afterTap(v, n))}
          >
            <StarIcon filled={n <= v} />
          </button>
        );
      })}
    </span>
  );
}

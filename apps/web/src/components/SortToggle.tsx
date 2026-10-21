/** "Date taken | Date added": the order of the library, remembered per user. */
export function SortToggle({ value, onChange }: { value: "taken" | "added"; onChange: (v: "taken" | "added") => void }) {
  return (
    <div className="sort-toggle" role="group" aria-label="Sort photos by">
      {(
        [
          ["taken", "Date taken"],
          ["added", "Date added"],
        ] as const
      ).map(([v, label]) => (
        <button
          key={v}
          type="button"
          className="sort-option"
          aria-pressed={value === v}
          onClick={() => value !== v && onChange(v)}
        >
          {label}
        </button>
      ))}
    </div>
  );
}

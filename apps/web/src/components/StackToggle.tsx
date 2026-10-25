/** "Stacked | All photos": whether bursts and stacks show as one tile in the timeline, remembered per user. */
export function StackToggle({ collapsed, onChange }: { collapsed: boolean; onChange: (collapsed: boolean) => void }) {
  return (
    <div className="sort-toggle stack-toggle" role="group" aria-label="Stacks">
      {(
        [
          [true, "Stacked"],
          [false, "Every photo"],
        ] as const
      ).map(([v, label]) => (
        <button
          key={label}
          type="button"
          className="sort-option"
          aria-pressed={collapsed === v}
          title={v ? "Bursts and stacks show as one tile" : "Every photo of a stack shows"}
          onClick={() => collapsed !== v && onChange(v)}
        >
          {label}
        </button>
      ))}
    </div>
  );
}

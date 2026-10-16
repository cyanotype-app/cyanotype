import type { ReactNode } from "react";

interface Props {
  count: number;
  /** How many items could be selected (the loaded ones); "Select all" is disabled when everything is selected. */
  total: number;
  onSelectAll: () => void;
  onClear: () => void;
  /** Leaves selection mode. Omit where selection is the page's only mode (Trash). */
  onDone?: () => void;
  /** The actions for the selection: "Move to trash" in the library, "Restore" / "Delete forever" in the trash.
   * Later features (add to album) add their button here; the bar itself does not change. */
  children?: ReactNode;
}

/** Sticky bar shown while selecting: count, select all / clear, and the page's actions. */
export function SelectionBar({ count, total, onSelectAll, onClear, onDone, children }: Props) {
  return (
    <div className="selection-bar" role="toolbar" aria-label="Selection">
      <span className="selection-count" aria-live="polite">
        {count === 0 ? "Select photos" : `${count.toLocaleString("en-US")} selected`}
      </span>
      <div className="selection-actions">
        <button
          type="button"
          className="btn btn-quiet selection-tool"
          onClick={onSelectAll}
          disabled={total === 0 || count === total}
        >
          Select all
        </button>
        <button type="button" className="btn btn-quiet selection-tool" onClick={onClear} disabled={count === 0}>
          Clear
        </button>
        {/* display: contents on wide screens (one row); its own full-width row on phones */}
        <div className="selection-primary">{children}</div>
        {onDone && (
          <button type="button" className="btn btn-quiet selection-tool" onClick={onDone}>
            Done
          </button>
        )}
      </div>
    </div>
  );
}

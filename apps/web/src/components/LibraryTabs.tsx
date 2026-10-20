import { useRef, useState } from "react";
import { NavLink } from "react-router-dom";
import { SearchBar } from "./SearchBar";

/**
 * All photos | Favorites | Map as pills under the top bar (three fit at 320 px), and the search field. On a phone the
 * field is an icon that takes the whole row while it is open, so the row never holds more than three pills and
 * one icon.
 */
export function LibraryTabs() {
  const [searching, setSearching] = useState(false);
  return (
    <div className="library-header" data-searching={searching}>
      <nav className="library-tabs" aria-label="Library views">
        <NavLink to="/library" end className="library-tab library-tab-all">
          All photos
        </NavLink>
        <NavLink to="/favorites" end className="library-tab">
          Favorites
        </NavLink>
        <NavLink to="/map" end className="library-tab">
          Map
        </NavLink>
        <LibraryMore />
      </nav>
      <SearchBar onOpenChange={setSearching} />
    </div>
  );
}

/** "More": the views that do not fit as pills (Recently added, On this day, Tags, Duplicates, Archive and Hidden). */
function LibraryMore() {
  const ref = useRef<HTMLDetailsElement>(null);
  const [open, setOpen] = useState(false);
  const close = () => ref.current?.removeAttribute("open");
  return (
    <details className="library-more" ref={ref} onToggle={(e) => setOpen(e.currentTarget.open)}>
      <summary className="library-tab" aria-label="More views">
        <span className="more-label">More</span>
        <span className="more-dots" aria-hidden="true">
          ⋯
        </span>
      </summary>
      {open && (
      <ul className="library-more-menu">
        {(
          [
            ["/recent", "Recently added"],
            ["/memories", "On this day"],
            ["/tags", "Tags"],
            ["/duplicates", "Duplicates"],
            ["/archive", "Archive"],
            ["/hidden", "Hidden"],
          ] as const
        ).map(([to, label]) => (
          <li key={to}>
            <NavLink to={to} end className="library-more-item" onClick={close}>
              {label}
            </NavLink>
          </li>
        ))}
      </ul>
      )}
    </details>
  );
}

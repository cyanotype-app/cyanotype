import { useRef, useState } from "react";
import { NavLink } from "react-router-dom";
import { usePeopleEnabled } from "../usePeople";

/**
 * All photos | Favorites | Map as pills under the top bar (three fit at 320 px). (Search has its own page now.)
 */
export function LibraryTabs() {
  return (
    <div className="library-header">
      <nav className="library-tabs" aria-label="Library views">
        <NavLink to="/photos" end className="library-tab library-tab-all">
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
    </div>
  );
}

/** "More": the views that do not fit as pills (Recently added, On this day, Tags, People when it is on, Duplicates, Archive and Hidden). */
function LibraryMore() {
  const ref = useRef<HTMLDetailsElement>(null);
  const [open, setOpen] = useState(false);
  const people = usePeopleEnabled() === true;
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
            ...(people ? ([["/people", "People"]] as const) : []),
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

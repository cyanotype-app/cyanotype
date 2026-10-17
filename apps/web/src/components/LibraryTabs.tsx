import { useState } from "react";
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
        <NavLink to="/library" end className="library-tab">
          All photos
        </NavLink>
        <NavLink to="/favorites" end className="library-tab">
          Favorites
        </NavLink>
        <NavLink to="/map" end className="library-tab">
          Map
        </NavLink>
      </nav>
      <SearchBar onOpenChange={setSearching} />
    </div>
  );
}

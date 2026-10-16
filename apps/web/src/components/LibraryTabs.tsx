import { NavLink } from "react-router-dom";

/** All photos | Favorites | Map: the views of the library, as pills under the top bar (three fit at 320 px). */
export function LibraryTabs() {
  return (
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
  );
}

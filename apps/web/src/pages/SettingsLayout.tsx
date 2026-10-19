import { NavLink, Outlet } from "react-router-dom";

/** Settings: Storage and API keys, with a small tab bar above whichever is open. */
export function SettingsLayout() {
  return (
    <>
      <nav className="admin-nav settings-nav" aria-label="Settings">
        <NavLink to="/settings/storage">Storage</NavLink>
        <NavLink to="/settings/sessions">Sessions</NavLink>
        <NavLink to="/settings/api-keys">API keys</NavLink>
      </nav>
      <Outlet />
    </>
  );
}

import { NavLink, Outlet } from "react-router-dom";
import { useAuth } from "../../auth";
import { NotFound } from "../NotFound";

/** The admin area. Members get the same page as any address that does not exist; the server refuses them anyway. */
export function AdminLayout() {
  const { state } = useAuth();
  if (state.status !== "authenticated" || state.user.role !== "admin") return <NotFound />;
  return (
    <div className="admin">
      <h1>Administration</h1>
      <nav className="admin-nav" aria-label="Administration">
        <NavLink to="/admin/accounts">Accounts</NavLink>
        <NavLink to="/admin/invites">Invites</NavLink>
      </nav>
      <Outlet />
    </div>
  );
}

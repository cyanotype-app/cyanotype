import { useEffect } from "react";
import { Link, Navigate, Outlet, Route, Routes, useLocation, useNavigate } from "react-router-dom";
import { useAuth } from "./auth";
import { Logo } from "./components/Logo";
import { AlbumPage } from "./pages/Album";
import { Albums } from "./pages/Albums";
import { ApiKeys } from "./pages/ApiKeys";
import { Library } from "./pages/Library";
import { Login } from "./pages/Login";
import { Register } from "./pages/Register";
import { Trash } from "./pages/Trash";
import { UploadProvider } from "./upload/UploadProvider";
import { UploadTray } from "./upload/UploadTray";

// Client routes must not start with /auth or /assets: the gateway sends those to the API.
export function App() {
  return (
    <Routes>
      <Route path="/login" element={<Login />} />
      <Route path="/register" element={<Register />} />
      <Route path="/logout" element={<Logout />} />
      <Route element={<RequireAuth />}>
        <Route path="/library" element={<Library />} />
        <Route path="/albums" element={<Albums />} />
        <Route path="/albums/:id" element={<AlbumPage />} />
        <Route path="/trash" element={<Trash />} />
        <Route path="/settings/api-keys" element={<ApiKeys />} />
      </Route>
      <Route path="*" element={<Navigate to="/library" replace />} />
    </Routes>
  );
}

function RequireAuth() {
  const { state, signOut } = useAuth();
  const loc = useLocation();
  if (state.status === "loading") return <p className="center muted" role="status">Loading…</p>;
  if (state.status === "anonymous") return <Navigate to="/login" replace state={{ from: loc.pathname }} />;
  return (
    <UploadProvider>
    <div className="shell">
      <header className="topbar">
        <Link to="/library" className="brand">
          <Logo size={26} />
          <span>Cyanotype</span>
        </Link>
        <div className="topbar-right">
          <Link to="/albums" className="btn btn-quiet">
            Albums
          </Link>
          <Link to="/trash" className="btn btn-quiet">
            Trash
          </Link>
          <Link to="/settings/api-keys" className="btn btn-quiet">
            API keys
          </Link>
          <span className="muted email">{state.user.email}</span>
          <button type="button" className="btn btn-quiet" onClick={() => void signOut()}>
            Sign out
          </button>
        </div>
      </header>
      <main>
        <Outlet />
      </main>
      <UploadTray />
    </div>
    </UploadProvider>
  );
}

function Logout() {
  const { signOut } = useAuth();
  const navigate = useNavigate();
  useEffect(() => {
    void signOut().finally(() => navigate("/login", { replace: true }));
  }, [signOut, navigate]);
  return <p className="center muted" role="status">Signing out…</p>;
}

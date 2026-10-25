import { useEffect } from "react";
import { Link, Navigate, Outlet, Route, Routes, useLocation, useNavigate } from "react-router-dom";
import { useAuth } from "./auth";
import { Logo } from "./components/Logo";
import { AlbumPage } from "./pages/Album";
import { Albums } from "./pages/Albums";
import { ApiKeys } from "./pages/ApiKeys";
import { AdminLayout } from "./pages/admin/AdminLayout";
import { Accounts } from "./pages/admin/Accounts";
import { TwoFactorAdmin } from "./pages/admin/TwoFactorAdmin";
import { Security } from "./pages/Security";
import { TakeoutImport } from "./pages/TakeoutImport";
import { AuditPage } from "./pages/admin/AuditPage";
import { HealthPage } from "./pages/admin/HealthPage";
import { Invites } from "./pages/admin/Invites";
import { Duplicates } from "./pages/Duplicates";
import { Frame } from "./pages/Frame";
import { HiddenPage } from "./pages/HiddenPage";
import { Library } from "./pages/Library";
import { MemoriesPage } from "./pages/MemoriesPage";
import { Login } from "./pages/Login";
import { MapPage } from "./pages/MapPage";
import { NotFound } from "./pages/NotFound";
import { PeoplePage } from "./pages/People";
import { PersonPage } from "./pages/PersonPage";
import { Register } from "./pages/Register";
import { ResetPassword } from "./pages/ResetPassword";
import { ShareView } from "./pages/ShareView";
import { SearchPage } from "./pages/Search";
import { Sessions } from "./pages/Sessions";
import { SettingsLayout } from "./pages/SettingsLayout";
import { TagPage } from "./pages/TagPage";
import { TagsPage } from "./pages/Tags";
import { SharedLinks } from "./pages/SharedLinks";
import { Storage } from "./pages/Storage";
import { Trash } from "./pages/Trash";
import { SlideshowProvider } from "./slideshow/SlideshowHost";
import { PeopleProvider } from "./usePeople";
import { UploadProvider } from "./upload/UploadProvider";
import { UploadTray } from "./upload/UploadTray";

// Client routes must not start with /auth or /assets: the gateway sends those to the API.
export function App() {
  return (
    <Routes>
      <Route path="/login" element={<Login />} />
      <Route path="/register" element={<Register />} />
      <Route path="/logout" element={<Logout />} />
      <Route path="/reset-password" element={<ResetPassword />} />
      {/* Public share links: no login, outside the signed-in shell. */}
      <Route path="/s/:token" element={<ShareView />} />
      {/* The photo frame: a wall tablet's view. Signed in, but outside the shell (no bar, nothing to click). */}
      <Route path="/frame" element={<Frame />} />
      <Route element={<RequireAuth />}>
        <Route path="/library" element={<Library key="all" />} />
        <Route path="/favorites" element={<Library key="favorites" favorites />} />
        <Route path="/recent" element={<Library key="recent" recent />} />
        <Route path="/archive" element={<Library key="archive" archive />} />
        <Route path="/hidden" element={<HiddenPage />} />
        <Route path="/memories" element={<MemoriesPage />} />
        <Route path="/map" element={<MapPage />} />
        <Route path="/search" element={<SearchPage />} />
        <Route path="/duplicates" element={<Duplicates />} />
        <Route path="/tags" element={<TagsPage />} />
        <Route path="/tags/:id" element={<TagPage />} />
        <Route path="/people" element={<PeoplePage />} />
        <Route path="/people/:id" element={<PersonPage />} />
        <Route path="/albums" element={<Albums />} />
        <Route path="/albums/:id" element={<AlbumPage />} />
        <Route path="/shared" element={<SharedLinks />} />
        <Route path="/trash" element={<Trash />} />
        <Route path="/settings" element={<SettingsLayout />}>
          <Route index element={<Navigate to="storage" replace />} />
          <Route path="storage" element={<Storage />} />
          <Route path="sessions" element={<Sessions />} />
          <Route path="security" element={<Security />} />
          <Route path="api-keys" element={<ApiKeys />} />
          <Route path="google-photos" element={<TakeoutImport />} />
          <Route path="*" element={<NotFound />} />
        </Route>
        <Route path="/admin" element={<AdminLayout />}>
          <Route index element={<Navigate to="accounts" replace />} />
          <Route path="accounts" element={<Accounts />} />
          <Route path="invites" element={<Invites />} />
          <Route path="security" element={<TwoFactorAdmin />} />
          <Route path="health" element={<HealthPage />} />
          <Route path="audit" element={<AuditPage />} />
          <Route path="*" element={<NotFound />} />
        </Route>
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
  // The administrator requires 2FA and this account has none: the server refuses everything else, so go and enrol.
  if (state.user.mfa_enrol_required && loc.pathname !== "/settings/security") {
    return <Navigate to="/settings/security" replace />;
  }
  return (
    <UploadProvider>
    <PeopleProvider>
    <SlideshowProvider>
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
          <Link to="/settings" className="btn btn-quiet">
            Settings
          </Link>
          {state.user.role === "admin" && (
            <Link to="/admin" className="btn btn-quiet">
              Admin
            </Link>
          )}
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
    </SlideshowProvider>
    </PeopleProvider>
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

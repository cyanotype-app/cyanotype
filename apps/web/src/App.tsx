import { useEffect } from "react";
import { Navigate, Outlet, Route, Routes, useLocation, useNavigate } from "react-router-dom";
import { useAuth } from "./auth";
import { AlbumPage } from "./pages/Album";
import { Albums } from "./pages/Albums";
import { Collections } from "./pages/Collections";
import { Activate } from "./pages/Activate";
import { ApiKeys } from "./pages/ApiKeys";
import { CliAuthorize } from "./pages/CliAuthorize";
import { AdminHome, AdminLayout } from "./pages/admin/AdminLayout";
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
import { SettingsHome, SettingsLayout } from "./pages/SettingsLayout";
import { DisplaySettings } from "./pages/DisplaySettings";
import { TagPage } from "./pages/TagPage";
import { TagsPage } from "./pages/Tags";
import { SharedLinks } from "./pages/SharedLinks";
import { Storage } from "./pages/Storage";
import { Trash } from "./pages/Trash";
import { SlideshowProvider } from "./slideshow/SlideshowHost";
import { PeopleProvider } from "./usePeople";
import { UploadProvider } from "./upload/UploadProvider";
import { UploadTray } from "./upload/UploadTray";
import { AppShell } from "./shell/AppShell";
import { UploadPickerProvider } from "./shell/UploadPicker";

// Client routes must not start with /auth or /assets: the gateway sends those to the API. /cli/authorize is the one client
// route under /cli (the rest of /cli is the CLI download); the gateway and the web container name it explicitly.
export function App() {
  return (
    <Routes>
      <Route path="/login" element={<Login />} />
      <Route path="/register" element={<Register />} />
      <Route path="/logout" element={<Logout />} />
      <Route path="/reset-password" element={<ResetPassword />} />
      {/* Approving a command-line login: sign in first if needed (and come back), no app shell around the question. */}
      <Route path="/cli/authorize" element={<CliAuthorize />} />
      <Route path="/activate" element={<Activate />} />
      {/* Public share links: no login, outside the signed-in shell. */}
      <Route path="/s/:token" element={<ShareView />} />
      {/* The photo frame: a wall tablet's view. Signed in, but outside the shell (no bar, nothing to click). */}
      <Route path="/frame" element={<Frame />} />
      <Route element={<RequireAuth />}>
        <Route path="/" element={<Collections />} />
        <Route path="/photos" element={<Library key="all" />} />
        {/* The grid's old address, from bookmarks and older links. */}
        <Route path="/library" element={<Navigate to="/photos" replace />} />
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
          <Route index element={<SettingsHome />} />
          <Route path="display" element={<DisplaySettings />} />
          <Route path="storage" element={<Storage />} />
          <Route path="sessions" element={<Sessions />} />
          <Route path="security" element={<Security />} />
          <Route path="api-keys" element={<ApiKeys />} />
          <Route path="google-photos" element={<TakeoutImport />} />
          <Route path="*" element={<NotFound />} />
        </Route>
        <Route path="/admin" element={<AdminLayout />}>
          <Route index element={<AdminHome />} />
          <Route path="accounts" element={<Accounts />} />
          <Route path="invites" element={<Invites />} />
          <Route path="security" element={<TwoFactorAdmin />} />
          <Route path="health" element={<HealthPage />} />
          <Route path="audit" element={<AuditPage />} />
          <Route path="*" element={<NotFound />} />
        </Route>
      </Route>
      <Route path="*" element={<Navigate to="/" replace />} />
    </Routes>
  );
}

function RequireAuth() {
  const { state } = useAuth();
  const loc = useLocation();
  if (state.status === "loading") return <p className="center muted" role="status">Loading…</p>;
  if (state.status === "anonymous") return <Navigate to="/login" replace state={{ from: loc.pathname + loc.search }} />;
  // The administrator requires 2FA and this account has none: the server refuses everything else, so go and enrol.
  if (state.user.mfa_enrol_required && loc.pathname !== "/settings/security") {
    return <Navigate to="/settings/security" replace state={{ from: loc.pathname + loc.search }} />;
  }
  return (
    <UploadProvider>
    <PeopleProvider>
    <SlideshowProvider>
    <UploadPickerProvider>
      <AppShell>
        <main>
          <Outlet />
        </main>
      </AppShell>
      <UploadTray />
    </UploadPickerProvider>
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

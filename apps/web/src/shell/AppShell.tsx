import { motion, useReducedMotion } from "motion/react";
import { FolderUp, ImageUp, LogOut, Plus, Settings, Shield } from "lucide-react";
import type { ReactNode } from "react";
import { Link, NavLink, useLocation } from "react-router-dom";
import { useAuth } from "../auth";
import { Logo } from "../components/Logo";
import { usePeopleEnabled } from "../usePeople";
import { useHasPets } from "../usePets";
import { Button } from "../ui/Button";
import { cn } from "../ui/cn";
import { Menu, MenuItem, MenuLabel, MenuSeparator } from "../ui/Menu";
import { LIBRARY, PRIMARY, UTILITIES, primaryFor, type NavItem } from "./nav";
import { useUploadPicker } from "./UploadPicker";

/**
 * The signed-in frame. On a phone: the page, then a frosted tab bar with Collections, Photos and Search. On a desktop:
 * a sidebar with those three, the library's views and the account. Pages draw their own header (`PageHeader`).
 */
export function AppShell({ children }: { children: ReactNode }) {
  return (
    <div className="app-shell min-h-dvh bg-bg text-fg md:grid md:grid-cols-[15.5rem_minmax(0,1fr)]">
      <Sidebar />
      <div className="app-main min-w-0 pb-tabbar">{children}</div>
      <TabBar />
    </div>
  );
}

function useVisible(items: readonly NavItem[]): readonly NavItem[] {
  const people = usePeopleEnabled() === true;
  const pets = useHasPets();
  return items.filter((i) => (!i.people || people) && (!i.pets || pets));
}

function Sidebar() {
  const library = useVisible(LIBRARY);
  return (
    <aside className="app-sidebar sticky top-0 hidden h-dvh flex-col gap-6 overflow-y-auto border-r border-line px-3 py-4 md:flex">
      <Link to="/" className="flex items-center gap-2.5 px-2.5 text-[0.9375rem] font-semibold tracking-tight text-fg no-underline">
        <Logo size={26} />
        Cyanotype
      </Link>
      <nav aria-label="Main" className="flex flex-col gap-0.5">
        {PRIMARY.map((i) => (
          <SideLink key={i.to} item={i} end={i.to === "/"} strong />
        ))}
      </nav>
      <UploadMenu />
      <nav aria-label="Library" className="flex flex-col gap-0.5">
        <p className="px-2.5 pb-1 text-xs font-medium text-muted">Library</p>
        {library.map((i) => (
          <SideLink key={i.to} item={i} />
        ))}
      </nav>
      <nav aria-label="Utilities" className="flex flex-col gap-0.5">
        <p className="px-2.5 pb-1 text-xs font-medium text-muted">Utilities</p>
        {UTILITIES.map((i) => (
          <SideLink key={i.to} item={i} />
        ))}
      </nav>
      <div className="mt-auto">
        <AccountMenu wide />
      </div>
    </aside>
  );
}

function SideLink({ item, end, strong }: { item: NavItem; end?: boolean; strong?: boolean }) {
  const { pathname } = useLocation();
  const Icon = item.icon;
  const active = strong ? primaryFor(pathname) === item.to && (item.to !== "/" || isCollectionsPath(pathname)) : undefined;
  return (
    <NavLink
      to={item.to}
      end={end}
      className={({ isActive }) =>
        cn(
          "flex h-9 items-center gap-3 rounded-[0.6rem] px-2.5 text-[0.9375rem] text-fg no-underline transition-colors",
          "hover:bg-surface [&_svg]:size-[1.15rem] [&_svg]:text-muted",
          (active ?? isActive) && "bg-surface font-medium [&_svg]:text-accent",
        )
      }
    >
      <Icon aria-hidden="true" />
      {item.label}
    </NavLink>
  );
}

/**
 * Collections is lit in the sidebar only on its own pages (home, a trip): a library view (Albums, Trash, …) is its own row,
 * and Settings or Admin belong to the account menu, not to any row.
 */
function isCollectionsPath(path: string): boolean {
  return path === "/" || path.startsWith("/trips/");
}

function TabBar() {
  const { pathname } = useLocation();
  const current = primaryFor(pathname);
  const reduce = useReducedMotion();
  return (
    <nav
      aria-label="Main"
      className="app-tabbar glass fixed inset-x-0 bottom-0 z-10 grid grid-cols-3 border-t border-line/70 pb-[env(safe-area-inset-bottom,0px)] md:hidden"
    >
      {PRIMARY.map((i) => {
        const Icon = i.icon;
        const on = current === i.to;
        return (
          <Link
            key={i.to}
            to={i.to}
            aria-current={on ? "page" : undefined}
            className={cn(
              "relative flex h-14 flex-col items-center justify-center gap-0.5 text-[0.6875rem] font-medium no-underline transition-colors",
              on ? "text-accent" : "text-muted",
            )}
          >
            {on && (
              <motion.span
                layoutId="tab-pill"
                aria-hidden="true"
                className="absolute top-1.5 h-8 w-14 rounded-full bg-accent/12"
                transition={reduce ? { duration: 0 } : { type: "spring", stiffness: 520, damping: 40 }}
              />
            )}
            <Icon className="relative size-[1.375rem]" strokeWidth={on ? 2.2 : 1.8} aria-hidden="true" />
            <span className="relative">{i.label}</span>
          </Link>
        );
      })}
    </nav>
  );
}

/** "Upload": photos or a whole folder, through the app's one picker. */
export function UploadMenu({ compact }: { compact?: boolean }) {
  const { pick } = useUploadPicker();
  return (
    <Menu
      align={compact ? "end" : "start"}
      label="Upload"
      trigger={
        compact ? (
          <Button variant="ghost" size="icon" aria-label="Upload">
            <Plus />
          </Button>
        ) : (
          <Button variant="primary" className="w-full">
            <Plus />
            Upload
          </Button>
        )
      }
    >
      <MenuItem onSelect={() => pick("files")}>
        <ImageUp />
        Photos and videos
      </MenuItem>
      <MenuItem onSelect={() => pick("folder")}>
        <FolderUp />
        A folder
      </MenuItem>
    </Menu>
  );
}

/** The account: settings, administration and signing out. A round initial on a phone, a row in the sidebar. */
export function AccountMenu({ wide }: { wide?: boolean }) {
  const { state, signOut } = useAuth();
  if (state.status !== "authenticated") return null;
  const { email, role } = state.user;
  const initial = (email.trim()[0] ?? "?").toUpperCase();
  const badge = (
    <span className="inline-flex size-8 shrink-0 items-center justify-center rounded-full bg-accent text-sm font-semibold text-on-accent">
      {initial}
    </span>
  );
  return (
    <Menu
      align={wide ? "start" : "end"}
      label="Account"
      trigger={
        wide ? (
          <button
            type="button"
            className="flex w-full items-center gap-2.5 rounded-[0.6rem] px-2 py-1.5 text-left hover:bg-surface"
          >
            {badge}
            <span className="min-w-0 flex-1 truncate text-sm">{email}</span>
          </button>
        ) : (
          <button type="button" aria-label="Account" className="rounded-full">
            {badge}
          </button>
        )
      }
    >
      <MenuLabel>{email}</MenuLabel>
      <MenuItem asChild>
        <Link to="/settings">
          <Settings />
          Settings
        </Link>
      </MenuItem>
      {role === "admin" && (
        <MenuItem asChild>
          <Link to="/admin">
            <Shield />
            Administration
          </Link>
        </MenuItem>
      )}
      <MenuSeparator />
      <MenuItem onSelect={() => void signOut()}>
        <LogOut />
        Sign out
      </MenuItem>
    </Menu>
  );
}

import {
  Archive,
  BookImage,
  Clock,
  Copy,
  EyeOff,
  Heart,
  Images,
  LayoutGrid,
  Link2,
  MapPin,
  Search,
  Sparkles,
  Tag,
  Trash2,
  Users,
  type LucideIcon,
} from "lucide-react";

export interface NavItem {
  to: string;
  label: string;
  icon: LucideIcon;
  /** Shown only when face recognition is on. */
  people?: boolean;
}

/** The three places the app is organised around: the tab bar on a phone, the top of the sidebar on a desktop. */
export const PRIMARY: readonly NavItem[] = [
  { to: "/", label: "Collections", icon: LayoutGrid },
  { to: "/photos", label: "Photos", icon: Images },
  { to: "/search", label: "Search", icon: Search },
];

/** Ways into the library. The sidebar lists them; on a phone they live on the Collections page. */
export const LIBRARY: readonly NavItem[] = [
  { to: "/albums", label: "Albums", icon: BookImage },
  { to: "/people", label: "People", icon: Users, people: true },
  { to: "/map", label: "Places", icon: MapPin },
  { to: "/memories", label: "Memories", icon: Sparkles },
  { to: "/favorites", label: "Favorites", icon: Heart },
  { to: "/recent", label: "Recently added", icon: Clock },
  { to: "/tags", label: "Tags", icon: Tag },
  { to: "/shared", label: "Shared links", icon: Link2 },
];

/** Housekeeping, at the bottom of the sidebar and of Collections. */
export const UTILITIES: readonly NavItem[] = [
  { to: "/archive", label: "Archive", icon: Archive },
  { to: "/hidden", label: "Hidden", icon: EyeOff },
  { to: "/duplicates", label: "Duplicates", icon: Copy },
  { to: "/trash", label: "Recently deleted", icon: Trash2 },
];

/** The primary tab a path belongs to, so `/albums/…` still lights Collections and `/photos` lights Photos. */
export function primaryFor(path: string): string {
  if (path === "/photos" || path.startsWith("/photos/")) return "/photos";
  if (path === "/search" || path.startsWith("/search/")) return "/search";
  return "/";
}

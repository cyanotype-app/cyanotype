import { FolderInput, HardDrive, KeyRound, MonitorSmartphone, ShieldCheck, SlidersHorizontal } from "lucide-react";
import { useAuth } from "../auth";
import { AreaHome, AreaLayout, type AreaGroup } from "../settings/SectionLayout";

/** Settings: what the library shows and stores, then the account's security. Each section is its own page on a phone. */
export const SETTINGS: readonly AreaGroup[] = [
  {
    title: "Library",
    sections: [
      { to: "display", label: "Display", icon: SlidersHorizontal, hint: "Memories, stacks and order" },
      { to: "storage", label: "Storage", icon: HardDrive, hint: "Space used and your limits" },
      { to: "google-photos", label: "Google Photos", icon: FolderInput, hint: "Move over a Takeout export" },
    ],
  },
  {
    title: "Account",
    sections: [
      { to: "security", label: "Security", icon: ShieldCheck, hint: "Two-factor authentication" },
      { to: "sessions", label: "Sessions", icon: MonitorSmartphone, hint: "Where you are signed in" },
      { to: "api-keys", label: "API keys", icon: KeyRound, hint: "For the command line" },
    ],
  },
];

export function SettingsLayout() {
  return <AreaLayout root="/settings" title="Settings" groups={SETTINGS} navLabel="Settings" />;
}

export function SettingsHome() {
  const { state } = useAuth();
  const email = state.status === "authenticated" ? state.user.email : null;
  return (
    <AreaHome
      root="/settings"
      groups={SETTINGS}
      top={
        email && (
          <p className="px-1 text-sm text-muted [overflow-wrap:anywhere]">
            Signed in as <span className="text-fg">{email}</span>
          </p>
        )
      }
    />
  );
}

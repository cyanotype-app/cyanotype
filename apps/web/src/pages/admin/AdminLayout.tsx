import { Activity, ScrollText, ShieldCheck, Ticket, Users } from "lucide-react";
import { useAuth } from "../../auth";
import { AreaHome, AreaLayout, type AreaGroup } from "../../settings/SectionLayout";
import { NotFound } from "../NotFound";

export const ADMIN: readonly AreaGroup[] = [
  {
    sections: [
      { to: "accounts", label: "Accounts", icon: Users, hint: "Who is here and what they use" },
      { to: "invites", label: "Invites", icon: Ticket, hint: "Links that let someone join" },
      { to: "security", label: "Security", icon: ShieldCheck, hint: "Two-factor authentication" },
      { to: "health", label: "Health", icon: Activity, hint: "Queue, backups and storage checks" },
      { to: "audit", label: "Audit log", icon: ScrollText, hint: "What admins did" },
    ],
  },
];

/** The admin area. Members get the same page as any address that does not exist; the server refuses them anyway. */
export function AdminLayout() {
  const { state } = useAuth();
  if (state.status !== "authenticated" || state.user.role !== "admin") return <NotFound />;
  return <AreaLayout root="/admin" title="Administration" groups={ADMIN} navLabel="Administration" wide />;
}

export function AdminHome() {
  return <AreaHome root="/admin" groups={ADMIN} />;
}

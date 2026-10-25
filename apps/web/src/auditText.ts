import type { AuditEntry } from "./api";
import { limitsWords } from "./limitsText";

const LABELS: Record<string, string> = {
  "invite.create": "Created an invite",
  "invite.reissue": "Made a new invite link",
  "invite.revoke": "Revoked an invite",
  "invite.use": "Registered with an invite",
  "storage.bucket_check": "Started a bucket check",
  "account.limits": "Changed an account's limits",
  "settings.default_limits": "Changed the default limits",
  "account.disable": "Disabled an account",
  "account.enable": "Enabled an account",
  "account.role": "Changed an account's role",
  "account.reset_link": "Made a password reset link",
  "account.password_reset": "Reset a password with a link",
  "account.sessions_revoked": "Signed an account out everywhere",
  "account.delete.requested": "Requested the deletion of an account",
  "account.delete.finished": "Finished deleting an account",
  "jobs.retry": "Retried failed jobs",
  "integrity.run": "Started a storage integrity check",
  "integrity.restore_version": "Restored a previous version of an original",
  "integrity.trash": "Moved a photo with a missing original to the trash",
  "integrity.acknowledge": "Marked a storage finding as lost or left it",
  "integrity.restore_photo": "Restored an untracked object as a photo",
  "integrity.delete_object": "Deleted an untracked object for good",
  "cli_login.approve": "Allowed a command-line login",
  "cli_login.deny": "Denied a command-line login",
  "cli_login.issue": "Signed the command line in (made an API key)",
  "account.api_key_revoked": "Revoked an API key of an account",
  "account.2fa_reset": "Reset an account's two-factor authentication",
  "2fa.require": "Required two-factor authentication for everyone",
  "2fa.unrequire": "Stopped requiring two-factor authentication",
};

/** A sentence for an action name; the name itself for one this version does not know. */
export const actionLabel = (action: string): string => LABELS[action] ?? action;

const isLimits = (v: unknown): v is Record<string, number | null> =>
  typeof v === "object" && v !== null && ("quota_bytes" in v || "daily_upload_bytes" in v || "max_file_bytes" in v);

const text = (v: unknown): string | null => {
  if (v === null || v === undefined || v === "") return null;
  if (typeof v === "string" || typeof v === "number" || typeof v === "boolean") return String(v);
  return null;
};

/** The facts of an entry as short lines: who it was about, from and to, limits before and after, counts. */
export function detailLines(e: Pick<AuditEntry, "detail">): string[] {
  const d = e.detail ?? {};
  const out: string[] = [];
  const email = text(d.email);
  if (email) out.push(email);
  const from = text(d.from);
  const to = text(d.to);
  if (from && to) out.push(`${from} → ${to}`);
  if (isLimits(d.before) && isLimits(d.after)) {
    out.push(`was: ${limitsWords(d.before)}`, `now: ${limitsWords(d.after)}`);
  }
  for (const [k, v] of Object.entries(d)) {
    if (["email", "from", "to", "before", "after"].includes(k)) continue;
    const t = text(v);
    if (t) out.push(`${k.replace(/_/g, " ")}: ${t}`);
  }
  return out;
}

/** A day (`YYYY-MM-DD`) for a date input from a time, in the browser's zone. */
export const dayOf = (iso: string): string => iso.slice(0, 10);

import type { ApiKey, ApiKeyScope } from "./api";

/** The manual-entry secret in groups of four (`ABCD EFGH ...`), easier to read and type than one long string. */
export const groupSecret = (secret: string): string => secret.replace(/\s+/g, "").replace(/(.{4})/g, "$1 ").trim();

/** A 6-digit code as typed, with spaces dropped; null when it is not six digits. */
export function cleanCode(input: string): string | null {
  const d = input.replace(/\s+/g, "");
  return /^\d{6}$/.test(d) ? d : null;
}

/** Whether `input` could be a recovery code (`abcd-efgh-2345-ijkl`, any case, dashes or spaces optional). */
export const looksLikeRecoveryCode = (input: string): boolean => /^[a-z2-7]{16}$/i.test(input.replace(/[\s-]/g, ""));

/** What is sent for a code field: six digits as digits, anything else as typed (a recovery code). */
export const codeToSend = (input: string): string => cleanCode(input) ?? input.trim();

/** The text file offered for download beside the recovery codes. Names the account and the server, never a secret. */
export function recoveryCodesFile(codes: string[], email: string, origin: string, now = new Date()): string {
  return [
    "Cyanotype recovery codes",
    `Account: ${email}`,
    `Server: ${origin}`,
    `Made: ${now.toISOString().slice(0, 10)}`,
    "",
    "Each code works once, in place of the 6-digit code from your authenticator app.",
    "Keep this file somewhere safe and private (a password manager is a good place).",
    "",
    ...codes,
    "",
  ].join("\n");
}

export const SCOPE_LABEL: Record<ApiKeyScope, string> = {
  import: "Import only",
  read: "Read only",
  full: "Full access",
};

export const SCOPE_HELP: Record<ApiKeyScope, string> = {
  import: "Upload photos and check status. Cannot see, change or delete anything. What the command line needs.",
  read: "See the library and download. Cannot upload or change anything.",
  full: "Everything you can do when signed in, except managing keys, two-factor authentication and administration.",
};

export const EXPIRY_CHOICES: { days: number | null; label: string }[] = [
  { days: null, label: "Never" },
  { days: 30, label: "30 days" },
  { days: 90, label: "90 days" },
  { days: 365, label: "1 year" },
];

/** "Expires 3 Mar 2027", "Expired 3 Mar 2026" or "No expiry". */
export function expiryLine(k: Pick<ApiKey, "expires_at" | "expired">): string {
  if (!k.expires_at) return "No expiry";
  const d = new Date(k.expires_at).toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" });
  return k.expired ? `Expired ${d}` : `Expires ${d}`;
}

/** The sentence for a failed code step. */
export function codeErrorText(status: number, code: string | undefined, fallback: string): string {
  if (code === "two_factor_locked") return "Too many wrong codes. Wait a few minutes and try again.";
  if (code === "challenge_expired") return "Your sign-in timed out. Enter your password again.";
  if (code === "invalid_code" || status === 401) return "That code is not right. Check the time on your phone and try the newest code.";
  if (status === 429) return "Too many attempts. Wait a minute and try again.";
  return fallback;
}

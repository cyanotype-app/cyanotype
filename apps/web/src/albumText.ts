export const photoCount = (n: number) => `${n.toLocaleString("en-US")} photo${n === 1 ? "" : "s"}`;

const month = (d: Date) => d.toLocaleDateString(undefined, { month: "short", year: "numeric" });

/** "Jun 2024" for one month, "Jun 2024 – Mar 2025" for a span; empty when the dates are missing. */
export function dateRange(first: string | null, last: string | null): string {
  const a = first ? new Date(first) : null;
  const b = last ? new Date(last) : null;
  if (!a || Number.isNaN(a.getTime())) return "";
  if (!b || Number.isNaN(b.getTime())) return month(a);
  const [from, to] = [month(a), month(b)];
  return from === to ? from : `${from} – ${to}`;
}

/** "2 hidden": members that are in the album but hidden (shown to the owner only, as a count). Empty when there are none. */
export const hiddenNote = (n: number | undefined) => (n && n > 0 ? `${n.toLocaleString("en-US")} hidden` : "");

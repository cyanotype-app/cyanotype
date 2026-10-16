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

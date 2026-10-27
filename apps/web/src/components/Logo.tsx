/**
 * The Cyanotype mark: a C open to the right with a sun in the middle (docs/brand/mark.svg,
 * same geometry). Drawn in `currentColor`, so it follows the surrounding text colour; give it
 * `text-accent` for the brand blue in either theme. The viewBox is cropped to the mark's
 * visible bounds (the source file keeps the full 100 x 100 box), so `size` is its real height.
 */
export function Logo({ size = 24, className }: { size?: number; className?: string }) {
  return (
    <svg width={size} height={size} viewBox="8 12 76 76" aria-hidden="true" focusable="false" className={className}>
      <path d="M73.5 28.5 A31 31 0 1 0 73.5 71.5" fill="none" stroke="currentColor" strokeWidth="13" strokeLinecap="round" />
      <circle cx="50" cy="50" r="10.5" fill="currentColor" />
    </svg>
  );
}

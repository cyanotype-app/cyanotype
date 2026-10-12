export function Logo({ size = 24 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 512 512" aria-hidden="true" focusable="false">
      <rect width="512" height="512" rx="96" fill="var(--pb-800)" />
      <g fill="none" stroke="var(--pb-100)" strokeWidth="26" strokeLinejoin="round">
        <rect x="116" y="156" width="280" height="200" rx="28" />
        <circle cx="256" cy="256" r="52" />
      </g>
      <circle cx="256" cy="256" r="20" fill="var(--pb-400)" />
    </svg>
  );
}

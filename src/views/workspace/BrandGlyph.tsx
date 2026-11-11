/**
 * The app icon's emerald-cut gem as a one-colour glyph for the title bar (like Zap's bolt and
 * Piw's nib). Same geometry as scripts/app-icon.svg, scaled to a 24px box.
 */
export function BrandGlyph({ size = 14 }: { size?: number }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={2}
      strokeLinejoin="round"
      strokeLinecap="round"
      aria-hidden
    >
      <polygon points="6.9,2 17.1,2 22,6.9 22,17.1 17.1,22 6.9,22 2,17.1 2,6.9" />
      <polygon
        points="8.7,6.9 15.3,6.9 17.1,8.7 17.1,15.3 15.3,17.1 8.7,17.1 6.9,15.3 6.9,8.7"
        strokeWidth={1.5}
      />
      <path d="M6.9 2 8.7 6.9M17.1 2l-1.8 4.9M6.9 22l1.8-4.9M17.1 22l-1.8-4.9" strokeWidth={1.5} />
    </svg>
  );
}

/**
 * The app icon's faceted diamond as a one-colour glyph for the title bar (like Zap's bolt and
 * Piw's nib). Same geometry as src-tauri/icons/AppIcon.icon, scaled to a 24px box with slightly
 * wider cuts so they stay visible at small sizes.
 */
export function BrandGlyph({ size = 14 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="currentColor" aria-hidden>
      <path d="M6.25 3.5h11.5L22 9H2z" />
      <path d="M2 11h3.2l4.6 10z" />
      <path d="M7.4 11h9.2L12 21z" />
      <path d="M22 11h-3.2l-4.6 10z" />
    </svg>
  );
}

/**
 * How many leading crumbs to fold into "…" so the rest fits in `available` pixels. The last
 * crumb always stays (it ellipsises on its own if even it does not fit).
 */
export function fitCrumbs(widths: readonly number[], available: number, ellipsis: number): number {
  const total = widths.reduce((sum, width) => sum + width, 0);
  if (total <= available || widths.length < 2) return 0;
  let rest = total;
  for (let hidden = 1; hidden < widths.length; hidden += 1) {
    rest -= widths[hidden - 1]!;
    if (ellipsis + rest <= available) return hidden;
  }
  return widths.length - 1;
}

/** How bright a colour looks (WCAG relative luminance, 0 to 1). Yellow is
 * bright and blue dark at the same HSL lightness, so decisions use this. */
export function luminance(r: number, g: number, b: number): number {
  const lin = (c: number) => (c /= 255) <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
}

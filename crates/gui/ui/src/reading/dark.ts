// Dark mode for a message written for a white page, as Outlook does: light
// backgrounds become dark and dark text becomes light, each keeping its hue.
// Pictures and emoji are left as they are, and colours that already suit a
// dark page (a blue button, white text on it) stay too.

/** Put in the frame before it loads, so the page doesn't flash white first. */
export const DARK_FIRST_CSS = "html,body{background:#121212}body{color:#e3e6eb}";

type Rgba = [number, number, number, number];

function parse(c: string): Rgba | null {
  const m = c.match(/rgba?\(([^)]+)\)/);
  if (!m) return null;
  const p = m[1]!.split(/[\s,/]+/).filter(Boolean).map(Number);
  if (p.length < 3 || p.some(Number.isNaN)) return null;
  return [p[0]!, p[1]!, p[2]!, p[3] ?? 1];
}

function toHsl([r, g, b]: Rgba): [number, number, number] {
  r /= 255; g /= 255; b /= 255;
  const max = Math.max(r, g, b), min = Math.min(r, g, b);
  const l = (max + min) / 2;
  if (max === min) return [0, 0, l];
  const d = max - min;
  const s = l > 0.5 ? d / (2 - max - min) : d / (max + min);
  const h = max === r ? (g - b) / d + (g < b ? 6 : 0) : max === g ? (b - r) / d + 2 : (r - g) / d + 4;
  return [h * 60, s, l];
}

const css = (h: number, s: number, l: number, a: number) =>
  `hsla(${h.toFixed(1)},${(s * 100).toFixed(1)}%,${(l * 100).toFixed(1)}%,${a})`;

/** A light background made dark: white becomes #121212, light grey a little lighter. */
function background(c: string): string | null {
  const v = parse(c);
  if (!v || v[3] === 0) return null;
  const [h, s, l] = toHsl(v);
  return l > 0.5 ? css(h, s, 0.07 + (1 - l) * 0.9, v[3]) : null;
}

/** Dark text made light: near black becomes near white, mid tones lighter. */
function text(c: string): string | null {
  const v = parse(c);
  if (!v || v[3] === 0) return null;
  const [h, s, l] = toHsl(v);
  return l < 0.5 ? css(h, s, Math.max(0.92 - l * 0.55, 0.7), v[3]) : null;
}

/** Recolours every element of `doc` for a dark page. */
export function darken(doc: Document): void {
  // Read every colour first, then write, so the page lays out once.
  const changes: [HTMLElement, string, string][] = [];
  for (const el of doc.querySelectorAll<HTMLElement>("body, body *")) {
    const cs = doc.defaultView!.getComputedStyle(el);
    const bg = background(cs.backgroundColor);
    if (bg) changes.push([el, "background-color", bg]);
    const fg = text(cs.color);
    if (fg) changes.push([el, "color", fg]);
    for (const side of ["top", "right", "bottom", "left"]) {
      if (cs.getPropertyValue(`border-${side}-style`) === "none") continue;
      const b = background(cs.getPropertyValue(`border-${side}-color`)) ?? text(cs.getPropertyValue(`border-${side}-color`));
      if (b) changes.push([el, `border-${side}-color`, b]);
    }
  }
  for (const [el, prop, value] of changes) el.style.setProperty(prop, value, "important");
}

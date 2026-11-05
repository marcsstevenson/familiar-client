// What the link under the pointer really points at, and whether its text
// claims somewhere else.

/** The host of a web link, or null. */
export function hostOf(url: string): string | null {
  try {
    const u = new URL(url);
    return u.protocol === "http:" || u.protocol === "https:" ? u.hostname.toLowerCase() : null;
  } catch {
    return null;
  }
}

/** A link's visible text, if it names a web host. */
function claimedHost(text: string): string | null {
  const t = text.trim();
  const m = /^(?:https?:\/\/)?((?:[a-z0-9-]+\.)+[a-z]{2,})(?:[/:?#]\S*)?$/i.exec(t);
  return m && m[1] ? m[1].toLowerCase() : null;
}

const bare = (h: string) => h.replace(/^www\./, "");

/**
 * When a link's text looks like an address on one site but the link goes to
 * another, returns the site it claims. `www.` differences don't count.
 */
export function mismatch(href: string, text: string): string | null {
  const real = hostOf(href);
  const claimed = claimedHost(text);
  if (!real || !claimed) return null;
  const r = bare(real);
  const c = bare(claimed);
  return r === c || r.endsWith("." + c) ? null : claimed;
}

import type { QuoteAction } from "../compose/spec";

const paths = {
  reply: ["M6 3.5L2 7.5l4 4", "M2.5 7.5H9a4.5 4.5 0 0 1 4.5 4.5v1"],
  replyAll: ["M5 3.5L1 7.5l4 4", "M8.5 3.5l-4 4 4 4", "M5 7.5h5a4 4 0 0 1 4 4v1.5"],
  forward: ["M10 3.5l4 4-4 4", "M13.5 7.5H7a4.5 4.5 0 0 0-4.5 4.5v1"],
};

export function replyIcon(action: QuoteAction, doc = document): SVGSVGElement {
  const ns = "http://www.w3.org/2000/svg";
  const svg = doc.createElementNS(ns, "svg");
  svg.setAttribute("viewBox", "0 0 16 16");
  svg.setAttribute("aria-hidden", "true");
  for (const d of paths[action]) {
    const path = doc.createElementNS(ns, "path");
    path.setAttribute("d", d);
    svg.append(path);
  }
  return svg;
}

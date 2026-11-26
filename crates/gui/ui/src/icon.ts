import { createEffect, createRoot, onCleanup } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import kea from "./assets/kea.svg?raw";
import { luminance } from "./colour";
import { accent, DEFAULT_ACCENT } from "./theme";

/** Transparent cutouts reveal the title bar. The native icon uses contrasting feathers. */
export function iconUrl(cutouts: "transparent" | "contrast" = "contrast"): string {
  const colour = accent() ?? DEFAULT_ACCENT;
  const channel = (i: number) => parseInt(colour.slice(i, i + 2), 16);
  const contrast = luminance(channel(1), channel(3), channel(5)) > 0.35 ? "#222" : "white";
  const svg = kea.replace("<svg ", `<svg style="color:${colour};--feather:${cutouts === "transparent" ? "transparent" : contrast}" `);
  return "data:image/svg+xml," + encodeURIComponent(svg);
}

/** Each native window gets the same rendering as the title bar. */
export function followWindowIcon(): void {
  createRoot(() => createEffect(() => {
    const image = new Image();
    image.src = iconUrl();
    // A native picker can stream colour changes as the pointer moves.
    let current = true;
    const timer = window.setTimeout(async () => {
      try {
        await image.decode();
        if (!current) return;
        const canvas = document.createElement("canvas");
        canvas.width = canvas.height = 64;
        const context = canvas.getContext("2d")!;
        context.drawImage(image, 0, 0, 64, 64);
        const rgba = context.getImageData(0, 0, 64, 64).data;
        await invoke("set_accent_icon", new Uint8Array(rgba.buffer));
      } catch (error) {
        console.warn("Could not update Familiar's window icon", error);
      }
    }, 120);
    onCleanup(() => { current = false; clearTimeout(timer); });
  }));
}

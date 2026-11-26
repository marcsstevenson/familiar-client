import { For, Show, createEffect, createSignal } from "solid-js";
import type { Theme } from "./api";
import { DEFAULT_ACCENT, accent, chooseAccent, chooseNeon, chooseTheme, neon, theme } from "./theme";

const PALETTE = [
  ["Familiar blue", DEFAULT_ACCENT], ["Windows blue", "#0078d4"], ["Sky", "#99d9ea"], ["Electric blue", "#007fff"],
  ["Indigo", "#5145cd"], ["Violet", "#8a5cf6"], ["Lavender", "#b69cff"], ["Slate", "#69797e"],
  ["Teal", "#008578"], ["Cyan", "#00b7c3"], ["Turquoise", "#40e0d0"], ["Mint", "#4cc2a1"],
  ["Forest", "#107c10"], ["Leaf", "#638446"], ["Lime", "#a3e635"], ["Lemon", "#f5df45"],
  ["Amber", "#d39734"], ["Orange", "#f7630c"], ["Coral", "#d96b5d"], ["Red", "#d13438"],
  ["Cherry", "#b51241"], ["Rose", "#c55486"], ["Hot pink", "#ff69b4"], ["Magenta", "#b146c2"],
] as const;

export function Appearance(props: { ref: (el: HTMLDialogElement) => void }) {
  let dialog!: HTMLDialogElement;
  const [hex, setHex] = createSignal("");
  const syncHex = () => setHex((accent() ?? DEFAULT_ACCENT).toUpperCase());
  createEffect(syncHex);
  const validHex = () => /^#?[0-9a-f]{6}$/i.test(hex().trim());

  function enterHex(text: string) {
    setHex(text);
    if (validHex()) selectColour("#" + text.trim().replace(/^#/, "").toLowerCase());
  }

  function selectColour(value: string) {
    chooseAccent(value.toLowerCase() === DEFAULT_ACCENT ? null : value);
    syncHex();
  }

  return (
    <dialog ref={el => { dialog = el; props.ref(el); }} class="appearance-dialog" aria-labelledby="appearance-title"
      onClose={syncHex} onKeyDown={e => e.stopPropagation()}>
      <h2 id="appearance-title">Appearance</h2>
      <div class="appearance-row">
        <label for="appearance-theme">Theme</label>
        <select id="appearance-theme" value={theme()} onChange={e => chooseTheme(e.currentTarget.value as Theme)}>
          <option value="system">Use system setting</option>
          <option value="light">Light</option>
          <option value="dark">Dark</option>
        </select>
      </div>
      <div id="accent-preset-label" class="accent-preset-label">Accent presets</div>
      <div class="accent-palette" role="group" aria-labelledby="accent-preset-label">
        <For each={PALETTE}>{([name, value]) =>
          <button type="button" class="accent-swatch" title={name} aria-label={name}
            aria-pressed={(accent() ?? DEFAULT_ACCENT).toLowerCase() === value}
            style={{ "--swatch": value }} onClick={() => selectColour(value)} />
        }</For>
      </div>
      <div class="appearance-row">
        <label for="appearance-accent">Accent colour</label>
        <div class="accent-inputs">
          <input id="appearance-accent" type="color" value={accent() ?? DEFAULT_ACCENT}
            onInput={e => selectColour(e.currentTarget.value)} />
          <input type="text" class="accent-hex" aria-label="Hex colour" spellcheck={false}
            aria-invalid={!validHex()} aria-describedby={!validHex() ? "accent-format" : undefined}
            value={hex()} onInput={e => enterHex(e.currentTarget.value)}
            onKeyDown={e => { if (e.key === "Enter" && validHex()) dialog.close(); }} />
        </div>
      </div>
      <Show when={!validHex()}>
        <p id="accent-format" class="accent-format">Enter six hexadecimal digits, for example #99D9EA.</p>
      </Show>
      <label class="appearance-row neon-option">
        <span>Neon</span>
        <input class="neon-switch" type="checkbox" role="switch" checked={neon()}
          onChange={e => chooseNeon(e.currentTarget.checked)} />
      </label>
      <footer>
        <button type="button" class="primary" disabled={!validHex()} onClick={() => dialog.close()}>Done</button>
      </footer>
    </dialog>
  );
}

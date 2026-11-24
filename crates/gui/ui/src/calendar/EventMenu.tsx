import { onCleanup, onMount, type JSX } from "solid-js";

export type MenuPoint = { x: number; y: number };

export function EventMenu(props: { point: MenuPoint; onClose: () => void; children: JSX.Element }) {
  let layer!: HTMLDialogElement;
  let menu!: HTMLDivElement;
  onMount(() => {
    layer.showModal();
    // The available actions can change once the event details arrive.
    const position = () => {
      menu.style.left = Math.max(0, Math.min(props.point.x, window.innerWidth - menu.offsetWidth)) + "px";
      menu.style.top = Math.max(0, Math.min(props.point.y, window.innerHeight - menu.offsetHeight)) + "px";
    };
    position();
    const observer = new ResizeObserver(position);
    observer.observe(menu);
    window.addEventListener("resize", position);
    onCleanup(() => { observer.disconnect(); window.removeEventListener("resize", position); });
  });

  return <dialog ref={layer} class="event-menu-layer" role="dialog" aria-label="Event actions"
    onCancel={e => { e.preventDefault(); props.onClose(); }}
    onMouseDown={e => { if (e.target === layer) props.onClose(); }}
    onContextMenu={e => { e.preventDefault(); if (e.target === layer) props.onClose(); }}>
    <div ref={menu} class="folder-menu" role="menu" aria-label="Event actions" onKeyDown={e => {
      if (e.key === "Tab") { e.preventDefault(); props.onClose(); return; }
      if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
      e.preventDefault();
      const items = Array.from(menu.querySelectorAll<HTMLButtonElement>("button:enabled"));
      const index = items.indexOf(document.activeElement as HTMLButtonElement);
      items[(index + (e.key === "ArrowDown" ? 1 : -1) + items.length) % items.length]?.focus();
    }}>{props.children}</div>
  </dialog>;
}

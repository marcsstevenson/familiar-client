// The search box above the list. Enter searches this device (the same query
// language as `fam mail search`); "Server too" also asks the server for
// message text that isn't on this device, and labels what only it found.
import { Show, createEffect, createSignal, on } from "solid-js";
import { isCommandError, run } from "../api";
import { notify, onlineHits, searchQuery, setOnlineHits, setSearchQuery } from "../state";

/** Focuses the search box (the `search` action). */
export const searchFocus = { focus: (): void => {} };

export function SearchBar() {
  let input!: HTMLInputElement;
  const [text, setText] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  searchFocus.focus = () => {
    input.focus();
    input.select();
  };
  // A smart folder sets the query; show it in the box.
  createEffect(on(searchQuery, (q) => {
    setText(q ?? "");
    setOnlineHits(null);
  }));

  async function searchServer() {
    const q = searchQuery();
    if (!q) return;
    setBusy(true);
    try {
      const out = await run("mail.search", { query: q, scope: "all", limit: 100 });
      // The search changed while the server was asked: drop the answer.
      if (searchQuery() !== q) return;
      for (const n of out.notices) notify(n);
      // Offline, famd not running or failed: the notices say why, and the
      // button still offers to try again.
      if (out.online !== "searched" && out.online !== "not_needed") return;
      setOnlineHits(
        out.hits
          .filter((h) => h.source === "online")
          .map((h) => ({
            subject: h.subject,
            from: h.from_name || h.from_addr,
            received_at: h.received_at,
            account: h.account,
          })),
      );
    } catch (e) {
      notify(isCommandError(e) ? e.message : String(e), true);
    } finally {
      setBusy(false);
    }
  }

  return (
    <form
      class="search-bar"
      role="search"
      onSubmit={(e) => {
        e.preventDefault();
        setSearchQuery(text().trim() || null);
      }}
    >
      <input
        ref={input}
        type="search"
        placeholder="Search mail  (from: subject: has:attachment …)"
        aria-label="Search mail"
        value={text()}
        onInput={(e) => setText(e.currentTarget.value)}
        onKeyDown={(e) => {
          if (e.key === "Escape") {
            e.preventDefault();
            setSearchQuery(null);
            input.blur();
          }
        }}
      />
      <Show when={searchQuery()}>
        <button type="button" class="search-server" disabled={busy()} onClick={searchServer}>
          {busy() ? "Searching…" : onlineHits() ? "Server searched" : "Server too"}
        </button>
      </Show>
    </form>
  );
}

/** Results only the server found, above the list. They aren't on this device, so they can't be opened yet. */
export function OnlineHits() {
  return (
    <Show when={(onlineHits()?.length ?? 0) > 0}>
      <details class="online-hits">
        <summary>{onlineHits()!.length} more on the server only</summary>
        <ul>
          {onlineHits()!.map((h) => (
            <li>
              <span class="online-label">online</span>
              <span class="row-from">{h.from}</span>
              <span class="row-subject">{h.subject}</span>
              <span class="row-date">{new Date(h.received_at).toLocaleDateString()}</span>
            </li>
          ))}
        </ul>
      </details>
    </Show>
  );
}

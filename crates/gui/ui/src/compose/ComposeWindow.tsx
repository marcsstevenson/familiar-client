// The compose window (WP 3.4): one draft per window. A draft is saved with the
// same `draft.*` commands as `fam draft`: created on the first save, then
// updated (only the parts changed since the last save), and sent with
// `draft.send`, which waits out the undo-send delay in famd's queue.
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { For, Show, createMemo, createSignal, onCleanup, onMount } from "solid-js";
import { type CommandError, isCommandError, run, type UiInfo } from "../api";
import { TitleBar } from "../TitleBar";
import { type Attachment, createdDraft, loadDraft } from "./draft";
import { PlainEditor, RichEditor, insertImage } from "./Editor";
import { bodyOf, htmlToText, isBlank, ownSignature, plainToRich, serialize, setSignature, showCids } from "./html";
import { type ComposeInfo, type FileInfo, type Identity, composeInfo, fileInfo, pickFiles } from "./ipc";
import { Recipients, addressOf } from "./Recipients";
import type { ComposeSpec } from "./spec";
import "./compose.css";

type SigChoice = "new" | "reply" | "none";
type Field = "from" | "to" | "cc" | "bcc" | "subject" | "body";

const MAX_IMAGE = 3 * 1024 * 1024;
const AUTOSAVE_MS = 4000;

function errorText(e: unknown): string {
  return isCommandError(e) ? e.message : String(e);
}

function sizeText(n: number): string {
  return n < 1024 ? `${n} B` : n < 1024 * 1024 ? `${Math.round(n / 1024)} KB` : `${(n / 1024 / 1024).toFixed(1)} MB`;
}

export function ComposeWindow(props: { info: UiInfo; spec: ComposeSpec }) {
  const win = getCurrentWindow();
  let editor: HTMLDivElement | undefined;

  const [settings, setSettings] = createSignal<ComposeInfo>({ identities: [], send_delay_seconds: 10, config_error: null });
  const [loading, setLoading] = createSignal(true);
  /** Days to wait for a reply before the sent message shows in Follow up; 0 for no reminder. */
  const [remindDays, setRemindDays] = createSignal(0);
  const [fatal, setFatal] = createSignal<string | null>(null);
  const [account, setAccount] = createSignal<string | null>(null);
  // An alias of the account to send from; null sends from its own address.
  const [from, setFrom] = createSignal<string | null>(null);
  const [isResponse, setIsResponse] = createSignal(props.spec.kind === "reply" || props.spec.kind === "forward");
  const [opened, setOpened] = createSignal(false); // an existing draft opened here
  const [to, setTo] = createSignal<string[]>([]);
  const [cc, setCc] = createSignal<string[]>([]);
  const [bcc, setBcc] = createSignal<string[]>([]);
  const [showCc, setShowCc] = createSignal(false);
  const [showBcc, setShowBcc] = createSignal(false);
  const [subject, setSubject] = createSignal("");
  const [mode, setMode] = createSignal<"rich" | "plain">("rich");
  const [plain, setPlain] = createSignal("");
  const [sig, setSig] = createSignal<SigChoice>(props.spec.kind === "new" ? "new" : "reply");
  const [attachments, setAttachments] = createSignal<Attachment[]>([]);
  const [draftId, setDraftId] = createSignal<number | null>(null);
  const [status, setStatus] = createSignal("");
  const [problem, setProblem] = createSignal<string | null>(null);
  const [dropping, setDropping] = createSignal(false);
  const [outside, setOutside] = createSignal<string | null>(null);
  const [sending, setSending] = createSignal<{ op: number; at: number; later?: Date } | null>(null);
  const [laterOpen, setLaterOpen] = createSignal(false);
  const [laterText, setLaterText] = createSignal("");
  // A send-later time waiting on the outside-recipients question.
  let pendingAt: Date | undefined;
  const [now, setNow] = createSignal(Date.now());
  // A save is waiting on the engine: the From account can't change under it.
  const [inFlight, setInFlight] = createSignal(false);

  // What changed since the last save, and attachments to take off.
  const edited = new Set<Field>();
  let removals: string[] = [];
  let generation = 0;
  let savedGeneration = 0;
  let saving: Promise<void> = Promise.resolve();
  let timer: number | undefined;
  let closing = false;

  const identity = (key = account()): Identity | undefined =>
    settings().identities.find((i) => i.key.toLowerCase() === key?.toLowerCase());
  const sigHtml = (choice = sig(), key = account()): string | null => {
    const id = identity(key);
    if (!id || choice === "none") return null;
    return (choice === "new" ? id.signature_new : id.signature_reply) ?? null;
  };
  const sigText = (choice = sig(), key = account()) => {
    const h = sigHtml(choice, key);
    return h ? htmlToText(h) : null;
  };
  const dirty = () => generation !== savedGeneration;
  // A saved draft can move to another account only by saving it afresh there,
  // which needs every attachment's file; a reply keeps its account.
  const canSwitchAccount = createMemo(
    () => !isResponse() && !opened() && attachments().every((a) => a.path !== undefined || draftId() === null),
  );

  const touch = (...fields: Field[]) => {
    fields.forEach((f) => edited.add(f));
    generation++;
    setStatus("");
    clearTimeout(timer);
    timer = window.setTimeout(() => void save(), AUTOSAVE_MS);
  };

  const body = (): { body: string; html: boolean } =>
    mode() === "rich" ? { body: serialize(editor!), html: true } : { body: plain(), html: false };
  const blank = () =>
    to().length + cc().length + bcc().length === 0 &&
    subject().trim() === "" &&
    attachments().length === 0 &&
    (mode() === "rich" ? isBlank(editor!) : plain().trim() === "" || plain().trim() === (sigText() ?? "").trim());

  /** Saves what changed. Saves run one after another. */
  function save(): Promise<void> {
    clearTimeout(timer);
    saving = saving.then(saveNow, saveNow);
    return saving;
  }

  async function saveNow(): Promise<void> {
    const gen = generation;
    const id = draftId();
    if (id !== null && !dirty()) return;
    if (id === null && blank()) return;
    const fresh = attachments().filter((a) => a.path !== undefined);
    const attach = fresh.map((a) => a.path!);
    const removing = removals;
    removals = [];
    setStatus("Saving…");
    setInFlight(true);
    try {
      if (id === null) {
        const out = await run("draft.create", {
          account: account(),
          from: from(),
          to: to(),
          cc: cc(),
          bcc: bcc(),
          subject: subject(),
          ...body(),
          attach,
          no_signature: true,
        });
        setDraftId(createdDraft(out));
      } else {
        const line = (v: string[]) => (v.length ? v : [""]);
        await run("draft.update", {
          draft: id,
          ...(edited.has("from") ? { from: from() ?? identity()?.email ?? null } : {}),
          ...(edited.has("to") ? { to: line(to()) } : {}),
          ...(edited.has("cc") ? { cc: line(cc()) } : {}),
          ...(edited.has("bcc") ? { bcc: line(bcc()) } : {}),
          ...(edited.has("subject") ? { subject: subject() } : {}),
          ...(edited.has("body") ? body() : {}),
          attach,
          remove_attachment: removing,
        });
      }
      // Files are on the draft now; they need no path from here on.
      setAttachments((list) => list.map((a) => (fresh.includes(a) ? { name: a.name, size: a.size } : a)));
      if (generation === gen) edited.clear();
      savedGeneration = gen;
      setProblem(null);
      setStatus("Saved");
    } catch (e) {
      removals = [...removing, ...removals];
      setStatus("");
      setProblem(`Not saved: ${errorText(e)}`);
      throw e;
    } finally {
      setInFlight(false);
    }
  }

  // --- opening -------------------------------------------------------------

  async function openDraft(id: number, fresh: boolean, source?: number) {
    const d = await loadDraft(id, bodyOf, source);
    setDraftId(id);
    setAccount(d.account);
    const own = identity(d.account)?.email;
    setFrom(own && d.from.toLowerCase() !== own.toLowerCase() ? d.from : null);
    setSubject(d.subject);
    setTo(d.to);
    setCc(d.cc);
    setBcc(d.bcc ?? []);
    setShowCc(d.cc.length > 0);
    setShowBcc((d.bcc ?? []).length > 0);
    setAttachments(d.attachments);
    // Swap in the images before the body is shown, so no `cid:` link is fetched.
    const doc = new DOMParser().parseFromString(d.html, "text/html");
    showCids(doc.body, d.images);
    editor!.innerHTML = doc.body.innerHTML;
    if (fresh) {
      // A new reply or forward: the signature goes above the quote, and is
      // saved straight away.
      setSignature(editor!, sigHtml());
      touch("body");
    } else {
      const s = ownSignature(editor!);
      const text = s ? htmlToText(s.innerHTML).trim() : null;
      setSig(!s ? "none" : text === sigText("reply")?.trim() ? "reply" : "new");
    }
  }

  onMount(async () => {
    try {
      const info = await composeInfo();
      setSettings(info);
      const spec = props.spec;
      if (spec.kind === "new") {
        const ids = info.identities;
        const pick = spec.account ? ids.find((i) => i.key === spec.account || i.email === spec.account) : ids[0];
        setAccount(pick?.key ?? spec.account ?? null);
        setTo(spec.to ?? []);
        editor!.innerHTML = "<div><br></div>";
        setSignature(editor!, sigHtml());
      } else if (spec.kind === "draft") {
        setOpened(true);
        await openDraft(spec.draft, false);
      } else {
        const out =
          spec.kind === "reply"
            ? await run("draft.reply", { message: spec.message, all: spec.all ?? false, body: "", html: true, no_signature: true })
            : await run("draft.forward", { message: spec.message, body: "", html: true, no_signature: true });
        const id = createdDraft(out);
        if (id === null) throw new Error("the draft wasn't made");
        setIsResponse(true);
        await openDraft(id, true, spec.message);
      }
      setLoading(false);
      placeCaret();
    } catch (e) {
      setFatal(errorText(e));
      setLoading(false);
    }
  });

  function placeCaret() {
    if (props.spec.kind === "new" && to().length === 0) return; // To has autofocus
    // The first line: the innermost leading <div>, above any signature.
    let first: Element | undefined = editor;
    while (first?.firstElementChild?.tagName === "DIV" && first.firstElementChild.id !== "Signature") first = first.firstElementChild;
    if (!first) return;
    editor!.focus();
    const r = document.createRange();
    r.setStart(first, 0);
    r.collapse(true);
    const sel = window.getSelection();
    sel?.removeAllRanges();
    sel?.addRange(r);
  }

  // --- identity and signature ---------------------------------------------

  /** A From choice: `<account key>\n<address>`. */
  const fromValue = (key: string, addr: string) => `${key}\n${addr}`;
  const fromChoices = () => settings().identities.flatMap((i) => [i.email, ...i.aliases].map((addr) => ({ i, addr })));

  function chooseFrom(value: string) {
    const [key, addr] = value.split("\n");
    if (key === undefined || addr === undefined) return;
    if (key !== account()) switchAccount(key);
    const own = identity()?.email.toLowerCase();
    const next = addr.toLowerCase() === own ? null : addr;
    if (next === from()) return;
    setFrom(next);
    touch("from");
  }

  function switchAccount(key: string) {
    const before = account();
    if (key === before) return;
    const oldSig = sigText(sig(), before);
    setAccount(key);
    setFrom(null);
    applySignature(oldSig);
    const id = draftId();
    if (id !== null) {
      // Saved under the old account: save afresh under the new one and take
      // the old draft away.
      setDraftId(null);
      void run("mail.delete", { messages: [id] }).catch((e) => setProblem(`The old draft is still in Drafts: ${errorText(e)}`));
    }
    touch("to", "cc", "bcc", "subject", "body");
  }

  /** Puts the current account's chosen signature in, replacing the old one. */
  function applySignature(oldText: string | null) {
    if (mode() === "rich") {
      setSignature(editor!, sigHtml());
    } else {
      const next = sigText() ?? "";
      const text = plain();
      if (oldText && text.includes(oldText)) setPlain(text.replace(oldText, next).replace(/\n+$/, next ? "" : "\n"));
      else if (next) setPlain(`${text.replace(/\n+$/, "")}\n\n${next}`);
    }
  }

  function chooseSignature(choice: SigChoice) {
    const oldText = sigText();
    setSig(choice);
    applySignature(oldText);
    touch("body");
  }

  function switchMode(next: "rich" | "plain") {
    if (next === mode()) return;
    if (next === "plain") {
      setPlain(htmlToText(serialize(editor!)));
      setMode("plain");
    } else {
      const html = plainToRich(plain(), sigText(), sigHtml());
      setMode("rich");
      editor!.innerHTML = html || "<div><br></div>";
    }
    touch("body");
  }

  // --- attachments and images ----------------------------------------------

  function addFiles(files: FileInfo[], at?: { x: number; y: number }) {
    const bad = files.filter((f) => f.problem);
    if (bad.length) setProblem(bad.map((f) => `${f.name}: ${f.problem}`).join("; "));
    const ok = files.filter((f) => !f.problem);
    const overBody = at && editor && mode() === "rich" && editor.contains(document.elementFromPoint(at.x, at.y));
    const images = overBody ? ok.filter((f) => f.image) : [];
    for (const f of images) insertImage(editor!, f.image!, at);
    const rest = ok.filter((f) => !images.includes(f));
    if (rest.length) setAttachments((list) => [...list, ...rest.map((f) => ({ name: f.name, size: f.size, path: f.path }))]);
    if (images.length || rest.length) touch("body");
  }

  function removeAttachment(a: Attachment) {
    setAttachments((list) => list.filter((x) => x !== a));
    if (a.path === undefined) removals.push(a.id ?? a.name);
    touch();
  }

  function pasteImage(file: File) {
    if (file.size > MAX_IMAGE) {
      setProblem(`${file.name || "That image"} is over 3 MB; attach it as a file instead`);
      return;
    }
    const r = new FileReader();
    r.onload = () => {
      insertImage(editor!, String(r.result));
      touch("body");
    };
    r.readAsDataURL(file);
  }

  async function pickPicture() {
    const files = await pickFiles();
    const images = files.filter((f) => f.image && !f.problem);
    images.forEach((f) => insertImage(editor!, f.image!));
    if (images.length) touch("body");
    if (images.length < files.length) setProblem("Only images of up to 3 MB go in the body; attach other files instead");
  }

  // --- sending --------------------------------------------------------------

  async function send(opts: { yes?: boolean; at?: Date } = {}) {
    setProblem(null);
    setLaterOpen(false);
    pendingAt = opts.at;
    if (to().length + cc().length + bcc().length === 0) {
      setProblem("Add someone to send it to");
      return;
    }
    const bad = [...to(), ...cc(), ...bcc()].filter((r) => !/^[^\s@<>:]+@[^\s@<>:]+\.[^\s@<>:]+$/.test(addressOf(r)));
    if (bad.length) {
      setProblem(`Check ${bad.join(", ")}`);
      return;
    }
    try {
      if (draftId() === null || dirty()) await save();
      const out = await run("draft.send", {
        draft: draftId()!,
        yes: opts.yes ?? false,
        ...(remindDays() > 0 ? { remind_days: remindDays() } : {}),
        ...(opts.at ? { at: opts.at.toISOString() } : {}),
      });
      const op = out.ops.find((o) => o.kind === "send_draft");
      if (op?.id && op.not_before) {
        setSending({ op: op.id, at: Date.parse(op.not_before), later: opts.at });
      } else {
        await finish();
      }
    } catch (e) {
      if (isCommandError(e) && (e as CommandError).kind === "confirmation_required") setOutside(e.message);
      else if (!problem()) setProblem(`Not sent: ${errorText(e)}`);
    }
  }

  /** Many outside people would see each other: put them in Bcc and yourself in To. */
  async function moveToBcc() {
    setOutside(null);
    setBcc([...bcc(), ...to(), ...cc()]);
    setTo([identity()?.email ?? ""].filter(Boolean));
    setCc([]);
    setShowBcc(true);
    touch("to", "cc", "bcc");
    await send({ at: pendingAt });
  }

  /** Send later: the server holds it and sends it at `at`, even with the PC off. */
  function sendLater(at: Date) {
    if (!(at.getTime() > Date.now() + 60_000)) {
      setProblem("Pick a time at least a minute from now");
      return;
    }
    void send({ at });
  }

  function sendLaterTyped() {
    const t = laterText();
    if (!t) {
      setProblem("Pick a date and time");
      return;
    }
    sendLater(new Date(t));
  }

  async function undoSend() {
    const s = sending();
    if (!s) return;
    try {
      await run("ops.cancel", { id: s.op });
      setSending(null);
      setStatus(s.later ? "Not scheduled; the draft is still in Drafts" : "Not sent; the draft is still in Drafts");
    } catch (e) {
      setProblem(`Too late to undo: ${errorText(e)}`);
    }
  }

  const tick = window.setInterval(() => {
    setNow(Date.now());
    const s = sending();
    if (s && Date.now() >= s.at) void finish();
  }, 250);
  onCleanup(() => {
    clearInterval(tick);
    clearTimeout(timer);
  });

  async function finish() {
    closing = true;
    await win.destroy();
  }

  async function discard() {
    clearTimeout(timer);
    const id = draftId();
    closing = true;
    try {
      await saving.catch(() => {});
      if (draftId() ?? id) await run("mail.delete", { messages: [(draftId() ?? id)!] });
    } catch (e) {
      closing = false;
      setProblem(`Not discarded: ${errorText(e)}`);
      return;
    }
    await win.destroy();
  }

  // --- window events --------------------------------------------------------

  onMount(async () => {
    const stopClose = await win.onCloseRequested(async (e) => {
      if (closing || fatal() || sending()) return;
      if (dirty() || (draftId() === null && !blank())) {
        e.preventDefault();
        try {
          await save();
          closing = true;
          await win.destroy();
        } catch {
          // The problem shows in the window; it stays open.
        }
      }
    });
    const stopDrop = await getCurrentWebview().onDragDropEvent(async (e) => {
      const p = e.payload;
      if (p.type === "enter" || p.type === "over") setDropping(true);
      else if (p.type === "leave") setDropping(false);
      else if (p.type === "drop") {
        setDropping(false);
        if (sending() || loading()) return;
        const scale = window.devicePixelRatio || 1;
        addFiles(await fileInfo(p.paths), { x: p.position.x / scale, y: p.position.y / scale });
      }
    });
    onCleanup(() => {
      stopClose();
      stopDrop();
    });
  });

  const onKeyDown = (e: KeyboardEvent) => {
    const mod = e.ctrlKey || e.metaKey;
    if (mod && e.key === "Enter") {
      e.preventDefault();
      void send();
    } else if (mod && e.key.toLowerCase() === "s") {
      e.preventDefault();
      void save().catch(() => {});
    }
  };

  const title = () => subject().trim() || (isResponse() ? "Reply" : "New message");
  const fromLabel = (i: Identity, addr = i.email) => (i.display_name ? `${i.display_name} <${addr}>` : addr);
  const busy = () => loading() || sending() !== null;

  return (
    <div class="app compose" onKeyDown={onKeyDown}>
      <TitleBar platform={props.info.platform} title={title()} />
      <main class="compose-main" aria-busy={loading()}>
        <Show when={fatal()}>
          {(f) => <div class="compose-fatal" role="alert">This message can't be opened: {f()}</div>}
        </Show>
        <div class="compose-actions">
          <button type="button" class="primary" disabled={busy() || !!fatal()} onClick={() => void send()} title="Send (Ctrl+Enter)">
            Send
          </button>
          <button type="button" disabled={busy() || !!fatal()} onClick={() => setLaterOpen(!laterOpen())} title="Send later: the server sends it at the time you pick, even with this PC off" aria-expanded={laterOpen()}>
            Later…
          </button>
          <button type="button" disabled={busy() || !!fatal()} onClick={() => void pickFiles().then((f) => addFiles(f))} title="Attach files">
            Attach
          </button>
          <button type="button" disabled={busy()} onClick={() => void discard()} title="Delete this draft">
            Discard
          </button>
          <span class="compose-spacer" />
          <label class="compose-select" title="If nobody replies in time, the message shows in Follow up">
            <span>Remind me</span>
            <select value={remindDays()} disabled={busy()} onChange={(e) => setRemindDays(Number(e.currentTarget.value))}>
              <option value="0">Never</option>
              <option value="1">No reply in 1 day</option>
              <option value="3">No reply in 3 days</option>
              <option value="7">No reply in 1 week</option>
            </select>
          </label>
          <label class="compose-select">
            <span>Format</span>
            <select value={mode()} disabled={busy()} onChange={(e) => switchMode(e.currentTarget.value as "rich" | "plain")}>
              <option value="rich">Rich text</option>
              <option value="plain">Plain text</option>
            </select>
          </label>
          <label class="compose-select">
            <span>Signature</span>
            <select value={sig()} disabled={busy()} onChange={(e) => chooseSignature(e.currentTarget.value as SigChoice)}>
              <option value="new" disabled={!identity()?.signature_new}>New message</option>
              <option value="reply" disabled={!identity()?.signature_reply}>Reply</option>
              <option value="none">None</option>
            </select>
          </label>
        </div>

        <div class="compose-fields">
          <div class="compose-field">
            <label class="compose-label" for="compose-from">From</label>
            <Show
              when={fromChoices().length > 1 || (account() && !identity())}
              fallback={<span class="compose-from">{identity() ? fromLabel(identity()!) : account() ?? "No account in config.toml"}</span>}
            >
              <select
                id="compose-from"
                value={identity() ? fromValue(identity()!.key, from() ?? identity()!.email) : account() ?? ""}
                disabled={busy() || inFlight()}
                title={canSwitchAccount() ? "" : "A reply or a saved draft stays on its account"}
                onChange={(e) => chooseFrom(e.currentTarget.value)}
              >
                <Show when={account() && !identity()}>
                  <option value={account()!}>{account()}</option>
                </Show>
                <For each={fromChoices()}>
                  {(c) => (
                    <option value={fromValue(c.i.key, c.addr)} disabled={c.i.key !== identity()?.key && !canSwitchAccount()}>
                      {fromLabel(c.i, c.addr)}
                    </option>
                  )}
                </For>
              </select>
            </Show>
            <span class="compose-spacer" />
            <Show when={!showCc()}>
              <button type="button" class="link" onClick={() => setShowCc(true)}>Cc</button>
            </Show>
            <Show when={!showBcc()}>
              <button type="button" class="link" onClick={() => setShowBcc(true)}>Bcc</button>
            </Show>
          </div>
          <Recipients label="To" value={to()} account={account() ?? undefined} autofocus={props.spec.kind === "new" && !props.spec.to?.length} onChange={(v) => { setTo(v); touch("to"); }} />
          <Show when={showCc()}>
            <Recipients label="Cc" value={cc()} account={account() ?? undefined} onChange={(v) => { setCc(v); touch("cc"); }} />
          </Show>
          <Show when={showBcc()}>
            <Recipients label="Bcc" value={bcc()} account={account() ?? undefined} onChange={(v) => { setBcc(v); touch("bcc"); }} />
          </Show>
          <div class="compose-field">
            <label class="compose-label" for="compose-subject">Subject</label>
            <input
              id="compose-subject"
              class="compose-subject"
              value={subject()}
              disabled={busy()}
              onInput={(e) => {
                setSubject(e.currentTarget.value);
                touch("subject");
              }}
            />
          </div>
        </div>

        <Show when={attachments().length > 0}>
          <ul class="attachments" aria-label="Attachments">
            <For each={attachments()}>
              {(a) => (
                <li class="attachment" title={a.path ?? a.name}>
                  <span class="attachment-name">{a.name}</span>
                  <span class="attachment-size">{sizeText(a.size)}</span>
                  <button type="button" class="chip-remove" aria-label={`Remove ${a.name}`} disabled={busy()} onClick={() => removeAttachment(a)}>
                    ×
                  </button>
                </li>
              )}
            </For>
          </ul>
        </Show>
        <Show when={props.spec.kind === "forward"}>
          <p class="compose-note">The original's attachments go with it.</p>
        </Show>

        <div class="compose-body" classList={{ hidden: mode() !== "rich" }}>
          <RichEditor ref={(el) => (editor = el)} onInput={() => touch("body")} onPasteImage={pasteImage} onPicture={() => void pickPicture()} disabled={busy()} />
        </div>
        <Show when={mode() === "plain"}>
          <div class="compose-body">
            <PlainEditor value={plain()} disabled={busy()} onInput={(v) => { setPlain(v); touch("body"); }} />
          </div>
        </Show>

        <Show when={dropping()}>
          <div class="drop-hint" aria-hidden="true">Drop on the text to put a picture in it, anywhere else to attach</div>
        </Show>

        <Show when={laterOpen()}>
          <div class="compose-dialog compose-later" role="dialog" aria-label="Send later">
            <p>Send later. The server holds it and sends it then, even with this PC off.</p>
            <div class="compose-dialog-buttons">
              <For each={laterChoices()}>
                {(c) => <button type="button" onClick={() => sendLater(c.at)}>{c.label}</button>}
              </For>
            </div>
            <div class="compose-dialog-buttons">
              <input type="datetime-local" aria-label="Date and time" value={laterText()} onInput={(e) => setLaterText(e.currentTarget.value)} />
              <button type="button" class="primary" onClick={sendLaterTyped}>Schedule</button>
              <button type="button" onClick={() => setLaterOpen(false)}>Cancel</button>
            </div>
          </div>
        </Show>

        <Show when={outside()}>
          {(m) => (
            <div class="compose-dialog" role="alertdialog" aria-label="Outside recipients">
              <p>{m()}</p>
              <p>Put them in Bcc so they can't see each other's addresses?</p>
              <div class="compose-dialog-buttons">
                <button type="button" class="primary" onClick={() => void moveToBcc()}>Move to Bcc and send</button>
                <button type="button" onClick={() => { setOutside(null); void send({ yes: true, at: pendingAt }); }}>Send anyway</button>
                <button type="button" onClick={() => setOutside(null)}>Cancel</button>
              </div>
            </div>
          )}
        </Show>
      </main>
      <footer class="statusbar" role="status">
        <Show when={sending()} fallback={<span classList={{ warn: !!problem() }}>{problem() ?? status() ?? ""}</span>}>
          {(s) => (
            <>
              <span>
                {s().later ? `Scheduling for ${whenLabel(s().later!)} in ` : "Sending in "}
                {Math.max(0, Math.ceil((s().at - now()) / 1000))} s
              </span>
              <button type="button" class="link" onClick={() => void undoSend()}>Undo</button>
            </>
          )}
        </Show>
        <span class="statusbar-spacer" />
        <Show when={settings().config_error}>{(e) => <span class="warn">Config: {e()}</span>}</Show>
      </footer>
    </div>
  );
}

/** The quick send-later choices: in an hour, tomorrow 8am, Monday 8am. */
function laterChoices(): { label: string; at: Date }[] {
  const now = new Date();
  const at8 = (days: number) => {
    const d = new Date(now);
    d.setDate(d.getDate() + days);
    d.setHours(8, 0, 0, 0);
    return d;
  };
  const toMonday = ((8 - now.getDay()) % 7) || 7;
  const out = [
    { label: "In 1 hour", at: new Date(now.getTime() + 3_600_000) },
    { label: "Tomorrow 8am", at: at8(1) },
  ];
  if (toMonday > 1) out.push({ label: "Monday 8am", at: at8(toMonday) });
  return out;
}

function whenLabel(d: Date): string {
  return d.toLocaleString(undefined, { weekday: "short", day: "numeric", month: "short", hour: "numeric", minute: "2-digit" });
}

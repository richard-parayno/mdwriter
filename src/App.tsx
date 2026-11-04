import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import CodeMirror from "@uiw/react-codemirror";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import { EditorView, lineNumbers } from "@codemirror/view";
import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { tags as t } from "@lezer/highlight";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  ArrowDown,
  ArrowUp,
  ChevronsUpDown,
  ExternalLink,
  Folder,
  GitPullRequest,
  ImagePlus,
  LoaderCircle,
  Pencil,
  Plus,
  RefreshCw,
  Settings as SettingsIcon,
  X,
} from "lucide-react";
import { activeProject, api, setApiProject, type Commit, type Conflict, type PostFiles, type PostSummary, type Settings } from "./api";
import { parsePost, serializePost, slugify, today, type Frontmatter, type Post } from "./frontmatter";
import { postUrl, SettingsPage } from "./SettingsPage";
import { livePreview } from "./livePreview";
import { changeMarkers } from "./changeMarkers";
import { ConflictView, ReviewView } from "./DiffViews";
import { Palette, type Cmd } from "./Palette";
import { Setup } from "./Setup";
import { bindings, display, fromEvent, recording, type ActionId } from "./keymap";

const highlight = syntaxHighlighting(
  HighlightStyle.define([
    { tag: t.heading, fontWeight: "700", color: "var(--fg)" },
    { tag: t.strong, fontWeight: "700" },
    { tag: t.emphasis, fontStyle: "italic" },
    { tag: t.strikethrough, textDecoration: "line-through" },
    { tag: [t.link, t.url], color: "var(--info)" },
    { tag: [t.processingInstruction, t.meta, t.contentSeparator], color: "var(--muted)" },
    { tag: t.monospace, color: "var(--warning)" },
    { tag: t.quote, color: "var(--muted)", fontStyle: "italic" },
  ]),
);
// GitHub-flavored, like Astro: strikethrough, tables, task lists.
const BASE_EXTENSIONS = [markdown({ base: markdownLanguage }), highlight, EditorView.lineWrapping];
const IMAGE_RE = /\.(png|jpe?g|gif|webp|avif|svg)$/i;

type Tone = "warning" | "info" | "success" | "muted" | "error";
type Label = { tone: Tone; label: string; rank: number };

/** One line of status per post, like a git client: what's local, what's behind, where it is on GitHub. */
function statusOf(p: PostSummary, hidden: boolean): Label {
  const pr = p.pr ? ` · PR #${p.pr.number}` : "";
  const live: Label = hidden ? { tone: "muted", label: "Hidden", rank: 6 } : { tone: "success", label: "Published", rank: 5 };
  if (!p.local) return p.pr ? { tone: "info", label: `PR #${p.pr.number} open`, rank: 4 } : live;
  if (p.conflict) return { tone: "error", label: "Conflict", rank: 0 };
  const s = p.sync;
  if (!s) return { tone: "muted", label: "Local", rank: 3 };
  if (!s.baseKnown) return { tone: "warning", label: "Pull needed", rank: 1 };
  if (!s.onRemote && !s.behind) return { tone: "warning", label: "Local only", rank: 2 };
  const parts = [];
  if (s.behind) parts.push(`↓ ${s.behind} behind`);
  if (s.changes) parts.push(`${s.changes} change${s.changes === 1 ? "" : "s"}`);
  if (parts.length) return { tone: s.behind ? "info" : "warning", label: parts.join(" · ") + pr, rank: s.behind ? 1 : 2 };
  if (p.pr) return { tone: "info", label: `PR #${p.pr.number} open`, rank: 4 };
  if (s.upstream.startsWith("post/")) return { tone: "info", label: "Pushed", rank: 4 };
  return live;
}

function ago(ms: number | null) {
  if (!ms) return "Never synced";
  const s = Math.round((Date.now() - ms) / 1000);
  if (s < 60) return "Synced just now";
  if (s < 3600) return `Synced ${Math.round(s / 60)}m ago`;
  if (s < 86400) return `Synced ${Math.round(s / 3600)}h ago`;
  return `Synced ${new Date(ms).toLocaleDateString()}`;
}

type Toast = { text: string; tone?: "error"; busy?: boolean; action?: { label: string; run: () => void } };
type Since = { rev: string; label: string };

export default function App() {
  const [posts, setPosts] = useState<PostSummary[]>([]);
  const [warnings, setWarnings] = useState<string[]>([]);
  const [syncing, setSyncing] = useState(false);
  const [syncedAt, setSyncedAt] = useState<number | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [selected, setSelected] = useState<string | null>(null);
  const [doc, setDoc] = useState<Post | null>(null);
  const [docRev, setDocRev] = useState(0);
  const [files, setFiles] = useState<PostFiles | null>(null);
  const [saveState, setSaveState] = useState<"saved" | "saving" | "error">("saved");
  const [dragging, setDragging] = useState(false);
  const [creating, setCreating] = useState(false);
  const [newTitle, setNewTitle] = useState("");
  const [newError, setNewError] = useState("");
  const [settings, setSettings] = useState<Settings | null>(null);
  const [showSettings, setShowSettings] = useState(false);
  const [busy, setBusy] = useState(false);
  const [toast, setToast] = useState<Toast | null>(null);
  const [conflict, setConflict] = useState<Conflict | null>(null);
  const [review, setReview] = useState<{ base: string; current: string; upstream: string } | null>(null);
  const [since, setSince] = useState<Since | null>(null);
  const [sinceText, setSinceText] = useState<{ key: string; text: string } | null>(null);
  const [history, setHistory] = useState<Commit[]>([]);
  /** The palette, open on every command or only on projects. */
  const [palette, setPalette] = useState<null | "all" | "projects">(null);
  const [adding, setAdding] = useState(false);
  const [, tick] = useState(0);

  const view = useRef<EditorView | null>(null);
  const docRef = useRef<Post | null>(null);
  const saveTimer = useRef<number | undefined>(undefined);
  const selectedRef = useRef<string | null>(null);
  /** The project posts are listed for; a list that comes back after a switch is dropped. */
  const projectRef = useRef("");

  const summary = posts.find((p) => p.slug === selected) ?? null;
  // Locked while a git action runs, so nothing typed during a pull is lost.
  const editable = !!summary?.local && !busy;
  const editableRef = useRef(false);
  editableRef.current = editable;
  selectedRef.current = selected;
  docRef.current = doc;

  /** Lists posts. Without `fetch` it reads local state only and returns at once. */
  const refresh = useCallback(async (fetch: boolean) => {
    const id = projectRef.current;
    if (!id) return [];
    if (fetch) setSyncing(true);
    try {
      const list = await api.listPosts(fetch);
      if (projectRef.current !== id) return [];
      setPosts(list.posts);
      setWarnings(list.warnings);
      setSyncedAt(list.syncedAt);
      return list.posts;
    } catch (e) {
      if (projectRef.current === id) setWarnings([String(e)]);
      return [];
    } finally {
      if (fetch) setSyncing(false);
    }
  }, []);

  const flushSave = useCallback(async () => {
    window.clearTimeout(saveTimer.current);
    saveTimer.current = undefined;
    const slug = selectedRef.current;
    const current = docRef.current;
    if (!slug || !current || !editableRef.current) return;
    setSaveState("saving");
    try {
      await api.saveDraft(slug, serializePost(current));
      setSaveState("saved");
    } catch {
      setSaveState("error");
    }
  }, []);

  const update = useCallback(
    (next: Post) => {
      setDoc(next);
      docRef.current = next;
      setSaveState("saving");
      window.clearTimeout(saveTimer.current);
      saveTimer.current = window.setTimeout(async () => {
        await flushSave();
        refresh(false);
      }, 500);
    },
    [flushSave, refresh],
  );

  /** Reads the draft from disk again, e.g. after a pull rewrote it. */
  const reloadDoc = useCallback(async (slug: string) => {
    if (selectedRef.current !== slug) return;
    setDoc(parsePost(await api.readDraft(slug)));
    setFiles(await api.postFiles(slug));
    setDocRev((r) => r + 1);
  }, []);

  const select = useCallback(
    async (slug: string, list = posts) => {
      if (saveTimer.current) await flushSave();
      const p = list.find((x) => x.slug === slug);
      if (!p) return;
      setSelected(slug);
      selectedRef.current = slug;
      setReview(null);
      setHistory([]);
      setSince((s) => (s && (s.rev === "base" || s.rev === "main") && p.local ? s : null));
      // Another post or project may be selected while these load; then drop what they return.
      const conflict = p.conflict ? await api.conflict(slug) : null;
      const raw = p.local ? await api.readDraft(slug) : p.content;
      const files = p.local ? await api.postFiles(slug) : null;
      if (selectedRef.current !== slug) return;
      setConflict(conflict);
      setDoc(parsePost(raw));
      setDocRev((r) => r + 1);
      setFiles(files);
      setSaveState("saved");
    },
    [posts, flushSave],
  );

  const open = useCallback((url: string) => {
    openUrl(url).catch((e) => setWarnings([`Could not open ${url}: ${e}`]));
  }, []);

  /** Runs one git action with a busy toast; its result or error replaces the toast. */
  const act = useCallback(async (text: string, f: () => Promise<Toast>) => {
    setBusy(true);
    setToast({ text, busy: true });
    try {
      setToast(await f());
    } catch (e) {
      setToast({ text: String(e).replace(/^Error: /, ""), tone: "error" });
    } finally {
      setBusy(false);
    }
  }, []);

  useEffect(() => {
    if (!toast || toast.busy || toast.tone === "error") return;
    const id = window.setTimeout(() => setToast(null), toast.action ? 8000 : 4000);
    return () => window.clearTimeout(id);
  }, [toast]);

  // Instant load from local state, then a read-only fetch in the background to refresh the counts.
  useEffect(() => {
    api.getSettings().then((s) => void showProject(s), (e) => setWarnings([String(e)]));
    const id = window.setInterval(() => tick((n) => n + 1), 30_000);
    return () => window.clearInterval(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // The backend asks for this on Cmd+Q and window close, so an edit still in its 500 ms wait isn't lost.
  useEffect(() => {
    const un = listen("flush-before-quit", () => void flushSave().finally(() => invoke("quit")));
    return () => void un.then((f) => f());
  }, [flushSave]);

  const insertAt = useCallback((text: string, pos?: number) => {
    const v = view.current;
    if (!v) return;
    const line = v.state.doc.lineAt(pos ?? v.state.selection.main.head);
    // Never split a line of text; insert after it.
    const at = line.text.trim() ? line.to : line.from;
    // Keep the image in its own paragraph: blank line before it unless one is already there.
    const prevBlank = line.number === 1 || !v.state.doc.line(line.number - 1).text.trim();
    const pre = line.text.trim() ? "\n\n" : prevBlank ? "" : "\n";
    const insert = `${pre}${text}\n`;
    v.dispatch({ changes: { from: at, insert }, selection: { anchor: at + insert.length } });
    v.focus();
  }, []);

  // Tauri delivers file drops as native paths, not HTML5 drop events.
  useEffect(() => {
    const un = getCurrentWebview().onDragDropEvent(async ({ payload }) => {
      if (payload.type === "leave") return setDragging(false);
      if (!editableRef.current) return;
      if (payload.type === "enter" || payload.type === "over") return setDragging(true);
      setDragging(false);
      const slug = selectedRef.current;
      const images = payload.paths.filter((p) => IMAGE_RE.test(p));
      if (!slug || !images.length) return;
      const v = view.current;
      const scale = window.devicePixelRatio || 1;
      const pos = v?.posAtCoords({ x: payload.position.x / scale, y: payload.position.y / scale }) ?? undefined;
      const refs: string[] = [];
      for (const path of images) {
        const name = await api.addImage(slug, path);
        refs.push(`![${altFrom(name)}](./${name})`);
      }
      insertAt(refs.join("\n\n"), pos);
      setFiles(await api.postFiles(slug));
    });
    return () => void un.then((f) => f());
  }, [insertAt]);

  // The text the change markers compare against.
  const sinceKey = since && selected ? `${selected}@${since.rev}` : null;
  useEffect(() => {
    if (!since || !selected || !sinceKey) return;
    let live = true;
    api.textAt(selected, since.rev).then(
      (text) => live && setSinceText({ key: sinceKey, text: text ?? "" }),
      (e) => live && setToast({ text: String(e), tone: "error" }),
    );
    return () => {
      live = false;
    };
  }, [since, selected, sinceKey, docRev]);
  const sinceDoc = sinceText && sinceText.key === sinceKey ? parsePost(sinceText.text) : null;

  // ---------- git actions ----------

  async function pull(slug: string) {
    await flushSave();
    await act("Pulling…", async () => {
      const out = await api.pull(slug);
      await refresh(false);
      if (out.kind === "conflict") {
        if (selectedRef.current === slug) setConflict(await api.conflict(slug));
        return { text: "Pull found conflicts. Your draft is unchanged until you resolve them." };
      }
      if (out.kind === "upToDate") return { text: "Already up to date" };
      await reloadDoc(slug);
      const n = out.files.length;
      return {
        text: n ? `Pulled changes to ${out.files.join(", ")}` : "Pulled; the draft now tracks the remote",
        action: { label: "Undo", run: () => void undo(slug) },
      };
    });
  }

  async function push(slug: string) {
    await flushSave();
    await act("Pushing…", async () => {
      await api.push(slug);
      const list = await refresh(false);
      const p = list.find((x) => x.slug === slug);
      if (p?.pr) return { text: `Pushed to PR #${p.pr.number}` };
      return { text: `Pushed post/${slug}`, action: { label: "Open PR", run: () => void openPr(slug) } };
    });
  }

  async function openPr(slug: string) {
    const d = selectedRef.current === slug ? docRef.current : null;
    const title = d?.fm.title || slug;
    const body = [d?.fm.description ?? "", "", `Published from mdwriter. Only \`${postsDir}/${slug}/\` changes in this PR.`]
      .join("\n")
      .trim();
    await act("Opening PR…", async () => {
      const pr = await api.openPr(slug, title, body);
      await refresh(false);
      return { text: `Opened PR #${pr.number}`, action: { label: "View", run: () => open(pr.url) } };
    });
  }

  async function undo(slug: string) {
    await act("Undoing…", async () => {
      await api.undoPull(slug);
      await refresh(false);
      await reloadDoc(slug);
      return { text: "Pull undone. The draft is exactly as it was." };
    });
  }

  async function edit(slug: string) {
    await act("Starting a draft…", async () => {
      await api.edit(slug);
      const list = await refresh(false);
      await select(slug, list);
      return { text: `Editing a draft of ${slug} from ${branch}` };
    });
  }

  async function resolve(slug: string, text: string | null, remoteImages: string[]) {
    await act("Resolving…", async () => {
      await api.resolve(slug, text, remoteImages);
      setConflict(null);
      await refresh(false);
      await reloadDoc(slug);
      return { text: "Conflict resolved. Push when ready.", action: { label: "Undo", run: () => void undo(slug) } };
    });
  }

  async function abort(slug: string) {
    await act("Aborting…", async () => {
      await api.abortConflict(slug);
      setConflict(null);
      await refresh(false);
      return { text: "Pull aborted. Your draft is unchanged." };
    });
  }

  async function reviewChanges(slug: string) {
    await flushSave();
    const base = (await api.textAt(slug, "base")) ?? "";
    const current = docRef.current ? serializePost(docRef.current) : "";
    setReview({ base, current, upstream: posts.find((p) => p.slug === slug)?.sync?.upstream ?? branch });
  }

  async function loadHistory() {
    if (selected && summary?.local && !history.length) setHistory(await api.history(selected).catch(() => []));
  }

  async function createPost() {
    const title = newTitle.trim();
    const slug = slugify(title);
    if (!slug) return setNewError("Enter a title");
    if (posts.some((p) => p.slug === slug)) return setNewError(`${slug} already exists`);
    const content = serializePost({
      fm: { title, description: "", pubDate: today(), tags: [], draft: false },
      body: "",
      extra: {},
    });
    try {
      await api.createDraft(slug, content);
    } catch (e) {
      return setNewError(String(e));
    }
    setCreating(false);
    setNewTitle("");
    setNewError("");
    const list = await refresh(false);
    await select(slug, list);
  }

  /** Shows the settings' active project: lists its posts from local state at once, then fetches. */
  async function showProject(next: Settings) {
    const id = activeProject(next)?.id ?? "";
    const same = id === projectRef.current;
    if (!same) await flushSave();
    setSettings(next);
    setAdding(false);
    projectRef.current = id;
    setApiProject(id);
    if (!same) {
      setSelected(null);
      selectedRef.current = null;
      setDoc(null);
      setPosts([]);
      setWarnings([]);
      setSyncedAt(null);
      setConflict(null);
      setReview(null);
      setSince(null);
      setSinceText(null);
      setHistory([]);
      setFiles(null);
      setCreating(false);
      // Toast actions such as Undo act on the project they came from.
      setToast(null);
      setLoaded(false);
    }
    const list = await refresh(false);
    if (projectRef.current !== id) return;
    setLoaded(true);
    const keep = (same && list.find((p) => p.slug === selectedRef.current)) || [...list].sort(byStatus)[0];
    if (keep) await select(keep.slug, list);
    else {
      setSelected(null);
      setDoc(null);
    }
    refresh(true);
  }

  async function settingsSaved(next: Settings) {
    setShowSettings(false);
    await showProject(next);
  }

  /** Saves a pending edit first: once archived, the project refuses writes. */
  async function archiveProject(id: string, archived: boolean) {
    await flushSave();
    await settingsSaved(await api.archiveProject(id, archived));
  }

  async function switchProject(id: string) {
    if (id === projectRef.current) return;
    if (busy) return setToast({ text: "Wait for the current action to finish before switching projects", tone: "error" });
    setShowSettings(false);
    try {
      await flushSave();
      await showProject(await api.switchProject(id));
    } catch (e) {
      setToast({ text: String(e), tone: "error" });
    }
  }

  const project = activeProject(settings);
  const projects = settings?.projects.filter((p) => !p.archived) ?? [];
  const branch = project?.branch ?? "main";
  const postsDir = project?.postsDir ?? "src/content/posts";
  const sorted = useMemo(() => [...posts].sort(byStatus), [posts]);
  const filesDir = files?.dir;
  const markerBody = sinceDoc?.body;
  const extensions = useMemo(() => {
    const ext = [...BASE_EXTENSIONS];
    if (settings?.livePreview) {
      // Remote images load directly; ./name images load from the draft folder (not for read-only posts).
      const resolveImg = (url: string) =>
        /^https?:\/\//.test(url) ? url : filesDir && !url.includes("..") ? convertFileSrc(`${filesDir}/${url.replace(/^\.\//, "")}`) : null;
      ext.push(livePreview(resolveImg));
    }
    if (markerBody !== undefined && since) ext.push(changeMarkers(markerBody, since.label));
    return ext;
  }, [settings?.livePreview, filesDir, markerBody, since]);

  const diffExtensions = useMemo(
    () => (settings?.lineNumbers ? [...BASE_EXTENSIONS, lineNumbers()] : BASE_EXTENSIONS),
    [settings?.lineNumbers],
  );
  const fm = doc?.fm;
  const setFm = (patch: Partial<Post["fm"]>) => doc && update({ ...doc, fm: { ...doc.fm, ...patch } });
  const status = summary ? statusOf(summary, !!fm?.draft) : null;
  const liveTitle = (p: PostSummary) =>
    p.slug === selected && fm ? fm.title || "Untitled" : parsePost(p.content).fm.title || p.slug;
  const fmChanged = (k: keyof Frontmatter) =>
    !!sinceDoc && !!fm && JSON.stringify(sinceDoc.fm[k]) !== JSON.stringify(fm[k]) ? "changed" : undefined;

  const sync = summary?.sync;
  const canPush = !!summary?.local && !summary.conflict && !!sync?.baseKnown && !sync.behind && sync.changes > 0;
  const pushWhy = !summary?.local
    ? ""
    : summary.conflict
      ? "Resolve the conflict first"
      : !sync?.baseKnown
        ? "Pull first: the remote version of this post is unknown here"
        : sync.behind
          ? `${sync.upstream} has changes you don't have. Pull first.`
          : !sync.changes
            ? "Nothing to push"
            : `Commit and push to post/${summary.slug}`;
  const canOpenPr = !!summary?.local && !summary.pr && !!sync?.upstream.startsWith("post/");
  const pullFirst = !!summary?.local && (!sync?.baseKnown || !!sync?.behind) && !summary.conflict;

  // ---------- commands and shortcuts ----------

  const keys = bindings(settings);
  const slug = summary?.slug;
  const commands: Cmd[] = [];
  const add = (c: Omit<Cmd, "id"> & { id?: string; action?: ActionId }, when = true) =>
    when && commands.push({ ...c, id: c.id ?? c.label, shortcut: c.action ? display(keys[c.action]) : undefined });
  if (slug && summary?.local && !busy) {
    add({ group: "Post", label: "Pull", action: "pull", run: () => void pull(slug) }, !summary.conflict);
    add({ group: "Post", label: "Push", action: "push", run: () => void push(slug) }, canPush);
    add({ group: "Post", label: "Open PR", action: "openPr", run: () => void openPr(slug) }, canOpenPr);
    add({ group: "Post", label: "Review changes", action: "review", run: () => void reviewChanges(slug) }, !!sync?.changes);
    add({ group: "Post", label: "Undo last pull", run: () => void undo(slug) });
    add({ group: "Post", label: since ? "Hide change markers" : "Show changes since last sync", action: "markers", run: () => toggleMarkers() });
    add({ group: "Post", label: `Show changes since ${branch}`, run: () => setSince({ rev: "main", label: branch }) }, summary.onMain);
  }
  if (slug && summary && !summary.local && !busy) add({ group: "Post", label: "Edit as draft", run: () => void edit(slug) });
  if (summary?.pr) add({ group: "Post", label: `View PR #${summary.pr.number} on GitHub`, run: () => open(summary.pr!.url) });
  const site = summary && summary.onMain && !fm?.draft ? postUrl(project, summary.slug) : null;
  if (site) add({ group: "Post", label: "Open on site", run: () => open(site) });
  add({ group: "App", label: "New post", action: "newPost", run: () => setCreating(true) });
  add({ group: "App", label: "Sync with GitHub", action: "sync", run: () => void refresh(true) });
  add({ group: "App", label: "Settings", action: "settings", run: () => setShowSettings(true) }, !!settings);
  const projectCommands: Cmd[] = [
    ...projects.map((p) => ({
      id: `project:${p.id}`,
      group: "Project",
      label: `Switch to ${p.repo}${projects.some((q) => q !== p && q.repo === p.repo) ? ` (${p.branch})` : ""}`,
      shortcut: p.id === project?.id ? "Current" : undefined,
      run: () => void switchProject(p.id),
    })),
    { id: "project:add", group: "Project", label: "Add project…", run: () => void flushSave().then(() => setAdding(true)) },
  ];
  commands.push(...projectCommands);
  for (const p of sorted) {
    add({ id: `go:${p.slug}`, group: "Go to", label: liveTitle(p), run: () => (setShowSettings(false), void select(p.slug)) });
  }

  function toggleMarkers() {
    setSince((s) => (s ? null : { rev: "base", label: "last sync" }));
  }

  const handlers = useRef<Record<ActionId, () => void>>(null!);
  handlers.current = {
    palette: () => setPalette((p) => (p ? null : "all")),
    switchProject: () => setPalette((p) => (p ? null : "projects")),
    newPost: () => project && setCreating(true),
    sync: () => void refresh(true),
    settings: () => settings && setShowSettings(true),
    pull: () => slug && summary?.local && !busy && !summary.conflict && void pull(slug),
    push: () => slug && canPush && !busy && void push(slug),
    openPr: () => slug && canOpenPr && !busy && void openPr(slug),
    review: () => slug && sync?.changes && void reviewChanges(slug),
    markers: () => summary?.local && toggleMarkers(),
  };
  const keysRef = useRef(keys);
  keysRef.current = keys;
  const blockedRef = useRef(false);
  blockedRef.current = !project || adding;
  useEffect(() => {
    // Capture phase, so the editor's own Mod-Shift-L and friends don't see these.
    const onKey = (e: KeyboardEvent) => {
      // Not while a shortcut is being recorded, or behind setup, where the post they'd act on is hidden.
      if (recording.current || blockedRef.current) return;
      const pressed = fromEvent(e);
      const action = pressed && (Object.keys(keysRef.current) as ActionId[]).find((a) => keysRef.current[a] === pressed);
      if (!action) return;
      e.preventDefault();
      e.stopPropagation();
      handlers.current[action]();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, []);

  // ---------- render ----------

  const main = () => {
    if (showSettings && settings)
      return <SettingsPage key={JSON.stringify(settings)} settings={settings} onSaved={settingsSaved} onArchive={archiveProject} onClose={() => setShowSettings(false)} />;
    if (!doc || !fm || !summary)
      return <div className="empty">{loaded ? `Select a post or create one${keys.newPost ? ` with ${display(keys.newPost)}` : ""}` : "Loading posts…"}</div>;
    if (conflict && summary.conflict && files)
      return (
        <ConflictView
          conflict={conflict}
          localDir={files.dir}
          extensions={diffExtensions}
          busy={busy}
          onResolve={(text, imgs) => void resolve(summary.slug, text, imgs)}
          onAbort={() => void abort(summary.slug)}
        />
      );
    if (review)
      return <ReviewView {...review} extensions={diffExtensions} onClose={() => setReview(null)} />;
    return (
      <>
        <section className="fm">
          <label className={fmChanged("title")}>title</label>
          <input value={fm.title} readOnly={!editable} onChange={(e) => setFm({ title: e.target.value })} />
          <label className={fmChanged("description")}>description</label>
          <input
            value={fm.description}
            readOnly={!editable}
            placeholder="One line for the blog index and the PR body"
            onChange={(e) => setFm({ description: e.target.value })}
          />
          <label className={fmChanged("tags")}>tags</label>
          <Tags value={fm.tags} readOnly={!editable} onChange={(tags) => setFm({ tags })} />
          <label className={fmChanged("pubDate") ?? fmChanged("draft")}>pubDate</label>
          <div className="row gap">
            <input
              type="date"
              className="date"
              value={fm.pubDate}
              readOnly={!editable}
              onChange={(e) => e.target.value && setFm({ pubDate: e.target.value })}
            />
            <label className="check">
              <input type="checkbox" checked={fm.draft} disabled={!editable} onChange={(e) => setFm({ draft: e.target.checked })} />
              draft <span className="muted">(hidden on the live site)</span>
            </label>
          </div>
        </section>

        {summary.local && files && (
          <section className="images">
            {files.images.map((name) => (
              <button key={name} className="img" title={`Insert ./${name}`} onClick={() => insertAt(`![${altFrom(name)}](./${name})`)}>
                <img src={convertFileSrc(`${files.dir}/${name}`)} alt="" />
                <span>{name}</span>
              </button>
            ))}
            <span className="hint">
              <ImagePlus size={13} /> Drop images on the editor to copy them beside index.md
            </span>
          </section>
        )}

        <section
          className={`editor ${dragging ? "dragging" : ""}`}
          style={settings ? ({ "--editor-font-size": `${settings.editorFontSize}px` } as React.CSSProperties) : undefined}
        >
          <CodeMirror
            key={`${selected}:${docRev}`}
            value={doc.body}
            editable={editable}
            basicSetup={{ lineNumbers: !!settings?.lineNumbers, foldGutter: false, highlightActiveLine: false, syntaxHighlighting: false }}
            extensions={extensions}
            onCreateEditor={(v) => (view.current = v)}
            onChange={(body) => docRef.current && update({ ...docRef.current, body })}
            placeholder="Start writing…"
            theme="none"
          />
          {dragging && <div className="drop">Drop to add images to this post</div>}
        </section>
      </>
    );
  };

  // First run, or every project archived: nothing works without a repository, so ask for one first.
  if (settings && (!project || adding))
    return <Setup settings={settings} onDone={settingsSaved} onCancel={project ? () => setAdding(false) : undefined} />;

  return (
    <div className="app">
      <aside className="side">
        <div className="drag" data-tauri-drag-region />
        <div className="side-head">
          <button
            className="project-btn"
            title={hint("Switch project", keys.switchProject)}
            onClick={() => setPalette("projects")}
          >
            <span className="project-name">{project?.repo.split("/")[1] ?? "mdwriter"}</span>
            {project && projects.filter((p) => p.repo === project.repo).length > 1 && <span className="muted">{project.branch}</span>}
            <ChevronsUpDown size={12} className="muted" />
          </button>
          <div className="row">
            <button className="icon-btn" title={hint("New post", keys.newPost)} onClick={() => setCreating((c) => !c)}>
              <Plus size={14} />
            </button>
            <button
              className="icon-btn"
              title={hint("Settings", keys.settings)}
              aria-pressed={showSettings}
              onClick={() => setShowSettings((s) => !s)}
              disabled={!settings}
            >
              <SettingsIcon size={13} />
            </button>
          </div>
        </div>
        {creating && (
          <form
            className="new-post"
            onSubmit={(e) => {
              e.preventDefault();
              createPost();
            }}
          >
            <input
              autoFocus
              placeholder="Post title"
              value={newTitle}
              onChange={(e) => {
                setNewTitle(e.target.value);
                setNewError("");
              }}
              onKeyDown={(e) => e.key === "Escape" && setCreating(false)}
            />
            <div className="row">
              <div className="hint grow">{newError || (newTitle ? `posts/${slugify(newTitle)}/index.md` : "Enter to create")}</div>
              <button type="submit" className="btn small" disabled={!slugify(newTitle)}>
                Create
              </button>
            </div>
          </form>
        )}
        <nav className="post-list">
          {sorted.map((p) => {
            const s = statusOf(p, p.slug === selected && fm ? fm.draft : parsePost(p.content).fm.draft);
            return (
              <button
                key={p.slug}
                className="post"
                aria-current={p.slug === selected && !showSettings}
                onClick={() => {
                  setShowSettings(false);
                  select(p.slug);
                }}
              >
                <span className="pt">{liveTitle(p)}</span>
                <span className="status" data-tone={s.tone}>
                  {s.label}
                </span>
              </button>
            );
          })}
          {!posts.length && syncing && <div className="hint pad">Cloning and syncing…</div>}
        </nav>
        {warnings.length > 0 && (
          <div className="warn" title={warnings.join("\n")}>
            {warnings[0]}
          </div>
        )}
        <button className="side-foot" title={hint("Fetch from GitHub. Only updates the counts; drafts never change.", keys.sync)} onClick={() => refresh(true)} disabled={syncing}>
          <RefreshCw size={12} className={syncing ? "spin" : ""} />
          {syncing ? "Syncing…" : ago(syncedAt)}
          <span className="spacer" />
          {keys.palette && <kbd>{display(keys.palette)}</kbd>}
        </button>
      </aside>

      <main className="main">
        <header className="bar" data-tauri-drag-region>
          <span className="bar-title" data-tauri-drag-region>
            {showSettings ? "Settings" : fm?.title || (summary ? "Untitled" : "mdwriter")}
          </span>
          {!showSettings && (
            <div className="bar-actions">
              {summary?.pr && (
                <button className="ghost-btn" onClick={() => open(summary.pr!.url)}>
                  <GitPullRequest size={13} /> PR #{summary.pr.number}
                </button>
              )}
              {/* Posts with draft: true are built out of the live site, so their URL 404s. */}
              {site && (
                <button className="ghost-btn" title={site} onClick={() => open(site)}>
                  <ExternalLink size={13} /> Site
                </button>
              )}
            </div>
          )}
        </header>

        {main()}

        {!showSettings && summary && doc && (
          <footer className="foot">
            {status && (
              <span className="status" data-tone={status.tone}>
                {status.label}
              </span>
            )}
            <span className="muted path">
              <Folder size={12} />
              {summary.local ? `${postsDir}/${summary.slug}/index.md` : `On ${branch} · read only`}
            </span>
            {summary.local && <span className="muted">{saveState === "saving" ? "Saving…" : saveState === "error" ? "Save failed" : "Saved"}</span>}
            <span className="spacer" />
            {summary.local && !summary.conflict && (
              <select
                className="since"
                title={hint("Mark lines changed since a commit", keys.markers)}
                value={since?.rev ?? ""}
                onFocus={loadHistory}
                onMouseDown={loadHistory}
                onChange={(e) => {
                  const rev = e.target.value;
                  const c = history.find((h) => h.sha === rev);
                  setSince(!rev ? null : rev === "base" ? { rev, label: "last sync" } : rev === "main" ? { rev, label: branch } : { rev, label: c ? rev.slice(0, 7) : rev });
                }}
              >
                <option value="">Changes: off</option>
                <option value="base">Since last sync</option>
                {summary.onMain && <option value="main">Since {branch}</option>}
                {history.map((c) => (
                  <option key={c.sha} value={c.sha}>
                    {c.sha.slice(0, 7)} · {new Date(c.date * 1000).toLocaleDateString()} · {c.subject}
                  </option>
                ))}
              </select>
            )}
            {summary.local && !summary.conflict && (
              <>
                {!!sync?.changes && !pullFirst && (
                  <button className="btn" onClick={() => reviewChanges(summary.slug)} disabled={busy}>
                    Review
                  </button>
                )}
                <button
                  className={`btn ${pullFirst ? "primary" : ""}`}
                  title={hint("Fetch and merge remote changes into the draft", keys.pull)}
                  onClick={() => pull(summary.slug)}
                  disabled={busy}
                >
                  <ArrowDown size={13} /> Pull{sync?.behind ? ` ${sync.behind}` : ""}
                </button>
                <button className={`btn ${canPush ? "primary" : ""}`} title={hint(pushWhy, keys.push)} onClick={() => push(summary.slug)} disabled={busy || !canPush}>
                  <ArrowUp size={13} /> Push
                </button>
                {canOpenPr && (
                  <button className={`btn ${!canPush ? "primary" : ""}`} onClick={() => openPr(summary.slug)} disabled={busy}>
                    <GitPullRequest size={13} /> Open PR
                  </button>
                )}
              </>
            )}
            {!summary.local && (
              <button className="btn primary" onClick={() => edit(summary.slug)} disabled={busy}>
                <Pencil size={13} /> Edit
              </button>
            )}
          </footer>
        )}
      </main>

      {toast && (
        <div className="toast" data-tone={toast.tone} role="status">
          {toast.busy && <LoaderCircle size={13} className="spin" />}
          <span className="grow">{toast.text}</span>
          {toast.action && (
            <button
              className="link-btn"
              onClick={() => {
                const run = toast.action!.run;
                setToast(null);
                run();
              }}
            >
              {toast.action.label}
            </button>
          )}
          {!toast.busy && (
            <button className="icon-btn" onClick={() => setToast(null)} aria-label="Dismiss">
              <X size={12} />
            </button>
          )}
        </div>
      )}
      {palette && (
        <Palette
          commands={palette === "projects" ? projectCommands : commands}
          placeholder={palette === "projects" ? "Switch to project" : undefined}
          onClose={() => setPalette(null)}
        />
      )}
    </div>
  );
}

function Tags({ value, readOnly, onChange }: { value: string[]; readOnly: boolean; onChange: (t: string[]) => void }) {
  const [draft, setDraft] = useState("");
  const commit = () => {
    const t = draft.trim();
    if (t && !value.includes(t)) onChange([...value, t]);
    setDraft("");
  };
  return (
    <div className="tags">
      {value.map((t) => (
        <span key={t} className="chip">
          {t}
          {!readOnly && (
            <button onClick={() => onChange(value.filter((x) => x !== t))} aria-label={`Remove ${t}`}>
              <X size={10} />
            </button>
          )}
        </span>
      ))}
      {!readOnly && (
        <input
          value={draft}
          placeholder={value.length ? "" : "Add a tag"}
          onChange={(e) => {
            const v = e.target.value;
            if (!v.includes(",")) return setDraft(v);
            const added = v.split(",").map((t) => t.trim()).filter((t) => t && !value.includes(t));
            onChange([...value, ...new Set(added)]);
            setDraft("");
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              commit();
            } else if (e.key === "Backspace" && !draft && value.length) onChange(value.slice(0, -1));
          }}
          onBlur={commit}
        />
      )}
    </div>
  );
}

/** A tooltip with the shortcut, when the action has one. */
function hint(text: string, binding: string) {
  return binding ? `${text} (${display(binding)})` : text;
}

function byStatus(a: PostSummary, b: PostSummary) {
  const fa = parsePost(a.content).fm;
  const fb = parsePost(b.content).fm;
  return statusOf(a, fa.draft).rank - statusOf(b, fb.draft).rank || fb.pubDate.localeCompare(fa.pubDate);
}

function altFrom(name: string) {
  return name.replace(/\.[^.]+$/, "").replace(/[-_]+/g, " ");
}

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import CodeMirror from "@uiw/react-codemirror";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import { EditorView } from "@codemirror/view";
import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { tags as t } from "@lezer/highlight";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  ArrowUp,
  CircleCheck,
  CircleX,
  ExternalLink,
  Folder,
  GitBranch,
  GitCommitHorizontal,
  GitPullRequest,
  ImagePlus,
  LoaderCircle,
  Plus,
  RefreshCw,
  Settings as SettingsIcon,
  Upload,
  X,
} from "lucide-react";
import { api, type PostFiles, type PostSummary, type Progress, type Settings, type StepId } from "./api";
import { parsePost, serializePost, slugify, today, type Post } from "./frontmatter";
import { postUrl, SettingsPage } from "./SettingsPage";
import { livePreview } from "./livePreview";

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
const IMAGE_RE = /\.(png|jpe?g|gif|webp|avif|svg)$/i;

const TONE: Record<PostSummary["status"], [string, string]> = {
  draft: ["warning", "Local draft"],
  review: ["info", "In review"],
  changed: ["warning", "Unpublished changes"],
  published: ["success", "Published"],
};
const ORDER = { draft: 0, changed: 1, review: 2, published: 3 };

/** A merged post with `draft: true` is on main but hidden on the live site. */
function tone(p: PostSummary, draft: boolean): [string, string] {
  return p.status === "published" && draft ? ["muted", "Merged · draft"] : TONE[p.status];
}

type StepState = { state: Progress["state"] | "todo"; detail: string };
type PublishRun = { slug: string; update: boolean; running: boolean; steps: Record<StepId, StepState>; error?: string };
const STEP_IDS: StepId[] = ["sync", "branch", "commit", "push", "pr"];
const freshSteps = () =>
  Object.fromEntries(STEP_IDS.map((s) => [s, { state: "todo", detail: "" }])) as Record<StepId, StepState>;

export default function App() {
  const [posts, setPosts] = useState<PostSummary[]>([]);
  const [warnings, setWarnings] = useState<string[]>([]);
  const [syncing, setSyncing] = useState(true);
  const [selected, setSelected] = useState<string | null>(null);
  const [doc, setDoc] = useState<Post | null>(null);
  const [files, setFiles] = useState<PostFiles | null>(null);
  const [saveState, setSaveState] = useState<"saved" | "saving" | "error">("saved");
  const [dragging, setDragging] = useState(false);
  const [run, setRun] = useState<PublishRun | null>(null);
  const [creating, setCreating] = useState(false);
  const [newTitle, setNewTitle] = useState("");
  const [newError, setNewError] = useState("");
  const [settings, setSettings] = useState<Settings | null>(null);
  const [showSettings, setShowSettings] = useState(false);

  const view = useRef<EditorView | null>(null);
  const docRef = useRef<Post | null>(null);
  const saveTimer = useRef<number | undefined>(undefined);
  const selectedRef = useRef<string | null>(null);

  const summary = posts.find((p) => p.slug === selected) ?? null;
  const editable = !!summary?.local;
  const editableRef = useRef(false);
  editableRef.current = editable;
  selectedRef.current = selected;
  docRef.current = doc;

  const refresh = useCallback(async (fetch: boolean) => {
    if (fetch) setSyncing(true);
    try {
      const list = await api.listPosts(fetch);
      setPosts(list.posts);
      setWarnings(list.warnings);
      return list.posts;
    } catch (e) {
      setWarnings([String(e)]);
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

  const select = useCallback(
    async (slug: string, list = posts) => {
      if (saveTimer.current) await flushSave();
      const p = list.find((x) => x.slug === slug);
      if (!p) return;
      setSelected(slug);
      selectedRef.current = slug;
      setRun((r) => (r?.slug === slug ? r : null));
      const raw = p.local ? await api.readDraft(slug) : p.content;
      setDoc(parsePost(raw));
      setFiles(p.local ? await api.postFiles(slug) : null);
      setSaveState("saved");
    },
    [posts, flushSave],
  );

  const open = useCallback((url: string) => {
    openUrl(url).catch((e) => setWarnings([`Could not open ${url}: ${e}`]));
  }, []);

  useEffect(() => {
    api.getSettings().then(setSettings, (e) => setWarnings([String(e)]));
    refresh(true).then((list) => {
      const first = [...list].sort(byStatus)[0];
      if (first) select(first.slug, list);
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // The backend asks for this on Cmd+Q and window close, so an edit still in its 500 ms wait isn't lost.
  useEffect(() => {
    const un = listen("flush-before-quit", () => void flushSave().finally(() => invoke("quit")));
    return () => void un.then((f) => f());
  }, [flushSave]);

  useEffect(() => {
    const un = listen<Progress>("publish-progress", ({ payload }) => {
      setRun((r) =>
        r && {
          ...r,
          steps: { ...r.steps, [payload.step]: { state: payload.state, detail: payload.detail } },
          error: payload.state === "error" ? payload.detail : r.error,
        },
      );
    });
    return () => void un.then((f) => f());
  }, []);

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

  async function publish() {
    if (!summary || !doc) return;
    await flushSave();
    const slug = summary.slug;
    setRun({ slug, update: !!summary.pr, running: true, steps: freshSteps() });
    const body = [
      doc.fm.description,
      "",
      `Published from mdwriter. Only \`${postsDir}/${slug}/\` changes in this PR.`,
    ].join("\n");
    try {
      await api.publish(slug, doc.fm.title || slug, body.trim());
      setRun((r) => r && { ...r, running: false });
    } catch (e) {
      setRun((r) => r && { ...r, running: false, error: r.error ?? String(e) });
    }
    await refresh(true);
  }

  async function settingsSaved(next: Settings) {
    setSettings(next);
    setShowSettings(false);
    const list = await refresh(true);
    const keep = list.find((p) => p.slug === selectedRef.current) ?? [...list].sort(byStatus)[0];
    if (keep) await select(keep.slug, list);
    else {
      setSelected(null);
      setDoc(null);
    }
  }

  const branch = settings?.branch ?? "main";
  const postsDir = settings?.postsDir ?? "src/content/posts";
  const sorted = useMemo(() => [...posts].sort(byStatus), [posts]);
  const filesDir = files?.dir;
  const extensions = useMemo(() => {
    // GitHub-flavored, like Astro: strikethrough, tables, task lists.
    const base = [markdown({ base: markdownLanguage }), highlight, EditorView.lineWrapping];
    if (!settings?.livePreview) return base;
    // Remote images load directly; ./name images load from the draft folder (not for read-only posts).
    const resolve = (url: string) =>
      /^https?:\/\//.test(url) ? url : filesDir && !url.includes("..") ? convertFileSrc(`${filesDir}/${url.replace(/^\.\//, "")}`) : null;
    return [...base, livePreview(resolve)];
  }, [settings?.livePreview, filesDir]);
  const fm = doc?.fm;
  const setFm = (patch: Partial<Post["fm"]>) => doc && update({ ...doc, fm: { ...doc.fm, ...patch } });
  const [statusTone, label] = summary ? tone(summary, !!fm?.draft) : ["", ""];
  const liveTitle = (p: PostSummary) =>
    p.slug === selected && fm ? fm.title || "Untitled" : parsePost(p.content).fm.title || p.slug;

  return (
    <div className="app">
      <aside className="side">
        <div className="drag" data-tauri-drag-region />
        <div className="side-head">
          <span className="label">Posts</span>
          <div className="row">
            <button className="icon-btn" title="Refresh from GitHub" onClick={() => refresh(true)} disabled={syncing}>
              <RefreshCw size={13} className={syncing ? "spin" : ""} />
            </button>
            <button className="icon-btn" title="New post" onClick={() => setCreating((c) => !c)}>
              <Plus size={14} />
            </button>
            <button
              className="icon-btn"
              title="Settings"
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
            const [t, l] = tone(p, p.slug === selected && fm ? fm.draft : parsePost(p.content).fm.draft);
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
                <span className="status" data-tone={t}>
                  {l}
                  {p.pr ? ` · #${p.pr.number}` : ""}
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
              {summary && (summary.status === "published" || summary.status === "changed") && !fm?.draft && postUrl(settings, summary.slug) && (
                <button className="ghost-btn" title={postUrl(settings, summary.slug)!} onClick={() => open(postUrl(settings, summary.slug)!)}>
                  <ExternalLink size={13} /> Site
                </button>
              )}
            </div>
          )}
        </header>

        {showSettings && settings ? (
          <SettingsPage key={JSON.stringify(settings)} settings={settings} onSaved={settingsSaved} onClose={() => setShowSettings(false)} />
        ) : doc && fm && summary ? (
          <>
            <section className="fm">
              <label>title</label>
              <input value={fm.title} readOnly={!editable} onChange={(e) => setFm({ title: e.target.value })} />
              <label>description</label>
              <input
                value={fm.description}
                readOnly={!editable}
                placeholder="One line for the blog index and the PR body"
                onChange={(e) => setFm({ description: e.target.value })}
              />
              <label>tags</label>
              <Tags value={fm.tags} readOnly={!editable} onChange={(tags) => setFm({ tags })} />
              <label>pubDate</label>
              <div className="row gap">
                <input
                  type="date"
                  className="date"
                  value={fm.pubDate}
                  readOnly={!editable}
                  onChange={(e) => e.target.value && setFm({ pubDate: e.target.value })}
                />
                <label className="check">
                  <input
                    type="checkbox"
                    checked={fm.draft}
                    disabled={!editable}
                    onChange={(e) => setFm({ draft: e.target.checked })}
                  />
                  draft <span className="muted">(hidden on the live site)</span>
                </label>
              </div>
            </section>

            {editable && files && (
              <section className="images">
                {files.images.map((name) => (
                  <button
                    key={name}
                    className="img"
                    title={`Insert ./${name}`}
                    onClick={() => insertAt(`![${altFrom(name)}](./${name})`)}
                  >
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
                key={selected}
                value={doc.body}
                editable={editable}
                basicSetup={{ lineNumbers: false, foldGutter: false, highlightActiveLine: false, syntaxHighlighting: false }}
                extensions={extensions}
                onCreateEditor={(v) => (view.current = v)}
                onChange={(body) => docRef.current && update({ ...docRef.current, body })}
                placeholder="Start writing…"
                theme="none"
              />
              {dragging && <div className="drop">Drop to add images to this post</div>}
            </section>

            {run?.slug === summary.slug && (
              <Steps run={run} slug={summary.slug} branch={branch} postsDir={postsDir} onClose={() => setRun(null)} />
            )}

            <footer className="foot">
              <span className="status" data-tone={statusTone}>
                {label}
                {summary.pr ? ` · PR #${summary.pr.number}` : ""}
              </span>
              <span className="muted path">
                <Folder size={12} />
                {summary.local ? `${postsDir}/${summary.slug}/index.md` : `On ${branch} · read only`}
              </span>
              {editable && <span className="muted">{saveState === "saving" ? "Saving…" : saveState === "error" ? "Save failed" : "Saved"}</span>}
              <span className="spacer" />
              <PublishButton summary={summary} running={!!run?.running} onClick={publish} />
            </footer>
          </>
        ) : (
          <div className="empty">{syncing ? "Loading posts…" : "Select a post or create one with +"}</div>
        )}
      </main>
    </div>
  );
}

function PublishButton({ summary, running, onClick }: { summary: PostSummary; running: boolean; onClick: () => void }) {
  if (running)
    return (
      <button className="btn primary" disabled>
        <LoaderCircle size={13} className="spin" /> Publishing
      </button>
    );
  if (!summary.local || summary.status === "published")
    return (
      <button className="btn" disabled>
        Merged
      </button>
    );
  if (summary.status === "review")
    return (
      <button className="btn" onClick={onClick} disabled={!summary.dirty} title={summary.dirty ? "" : "No local changes since the last push"}>
        <ArrowUp size={13} /> {summary.dirty ? `Push changes to #${summary.pr!.number}` : `Up to date with #${summary.pr!.number}`}
      </button>
    );
  return (
    <button className="btn primary" onClick={onClick}>
      <GitPullRequest size={13} /> {summary.status === "changed" ? "Publish changes as PR" : "Publish as PR"}
    </button>
  );
}

function Steps({
  run,
  slug,
  branch,
  postsDir,
  onClose,
}: {
  run: PublishRun;
  slug: string;
  branch: string;
  postsDir: string;
  onClose: () => void;
}) {
  const { update } = run;
  const text: Record<StepId, React.ReactNode> = {
    sync: <>Fetch the latest <code>origin</code> into mdwriter's own clone</>,
    branch: update ? <>Check out <code>post/{slug}</code> from its PR</> : <>Create <code>post/{slug}</code> from <code>origin/{branch}</code></>,
    commit: <>Commit only <code>{postsDir}/{slug}/</code>: <code>post: {slug}</code></>,
    push: <>Push <code>post/{slug}</code></>,
    pr: update ? <>Update the open PR</> : <>Open a PR with the description as its body</>,
  };
  const icons: Record<StepId, React.ReactNode> = {
    sync: <RefreshCw size={13} />,
    branch: <GitBranch size={13} />,
    commit: <GitCommitHorizontal size={13} />,
    push: <Upload size={13} />,
    pr: <GitPullRequest size={13} />,
  };
  return (
    <section className="steps" aria-live="polite">
      <div className="row">
        <span className="label">{run.running ? "Publishing…" : run.error ? "Publish failed" : "Published"}</span>
        <span className="spacer" />
        {!run.running && (
          <button className="icon-btn" onClick={onClose} title="Dismiss">
            <X size={13} />
          </button>
        )}
      </div>
      {STEP_IDS.map((id) => {
        const s = run.steps[id];
        return (
          <div key={id} className={`step ${s.state}`}>
            <span className="ic">
              {s.state === "done" ? <CircleCheck size={13} /> : s.state === "error" ? <CircleX size={13} /> : s.state === "active" ? <LoaderCircle size={13} className="spin" /> : icons[id]}
            </span>
            <span>
              {text[id]}
              {s.detail && <span className="muted"> — {s.detail}</span>}
            </span>
          </div>
        );
      })}
      {run.error && !Object.values(run.steps).some((s) => s.state === "error") && <div className="step error">{run.error}</div>}
    </section>
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

function byStatus(a: PostSummary, b: PostSummary) {
  return (
    ORDER[a.status] - ORDER[b.status] ||
    parsePost(b.content).fm.pubDate.localeCompare(parsePost(a.content).fm.pubDate)
  );
}

function altFrom(name: string) {
  return name.replace(/\.[^.]+$/, "").replace(/[-_]+/g, " ");
}

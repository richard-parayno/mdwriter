import { useEffect, useState } from "react";
import { Archive, CircleAlert, CircleCheck, Cloud, LoaderCircle, Wand2 } from "lucide-react";
import { activeProject, api, emptyProject, type BackupStatus, type Project, type Settings, type SiteDetection } from "./api";
import { Field } from "./SettingsPage";

/**
 * Connects a repository, then finds the live post URL. Shown on first run, when every project is archived,
 * and for Add project (with `onCancel`).
 */
export function Setup({ settings, onDone, onCancel }: { settings: Settings; onDone: (s: Settings) => void; onCancel?: () => void }) {
  const [form, setForm] = useState<Project>(emptyProject);
  const [saved, setSaved] = useState<Settings | null>(null);
  const [busy, setBusy] = useState(false);
  const [detecting, setDetecting] = useState(false);
  const [error, setError] = useState("");
  const [detected, setDetected] = useState<SiteDetection | null>(null);
  const [backup, setBackup] = useState<BackupStatus | null>(null);
  const step = saved ? "site" : "repo";
  const archived = settings.projects.filter((p) => p.archived);
  const set = (patch: Partial<Project>) => {
    setForm((f) => ({ ...f, ...patch }));
    setError("");
  };

  useEffect(() => {
    if (!onCancel) api.backupStatus().then(setBackup, () => {});
  }, [onCancel]);

  async function detect(p: Project) {
    setDetecting(true);
    setDetected(null);
    setError("");
    try {
      const d = await api.detectSite(p.branch, p.postsDir, p.id);
      setDetected(d);
      setForm((f) => ({ ...f, postUrl: d.url }));
    } catch (e) {
      setError(String(e));
    } finally {
      setDetecting(false);
    }
  }

  async function connect() {
    setBusy(true);
    setError("");
    try {
      // Checks the repository and branch are reachable, then adds the project and makes it active.
      const next = await api.addProject({ ...form, postUrl: "" });
      const added = activeProject(next)!;
      setSaved(next);
      setForm(added);
      void detect(added);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function finish(postUrl: string) {
    if (!saved) return;
    setBusy(true);
    try {
      const projects = saved.projects.map((p) => (p.id === form.id ? { ...form, postUrl } : p));
      onDone(await api.saveSettings({ ...saved, projects }));
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  }

  async function run(f: () => Promise<Settings>) {
    setBusy(true);
    try {
      onDone(await f());
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  }

  return (
    <div className="setup">
      <div className="drag setup-drag" data-tauri-drag-region />
      <form
        className="setup-card"
        onSubmit={(e) => {
          e.preventDefault();
          if (step === "repo") void connect();
          else void finish(form.postUrl);
        }}
        onKeyDown={(e) => e.key === "Escape" && step === "repo" && onCancel?.()}
      >
        <div className="setup-steps">
          <span aria-current={step === "repo"}>1 · Repository</span>
          <span aria-current={step === "site"}>2 · Site</span>
        </div>

        {step === "repo" ? (
          <>
            <h1>{onCancel ? "Add a project" : "Connect your site's repository"}</h1>
            <p className="muted">
              {onCancel
                ? "Each project is one repository, with its own drafts. Switch between them from the sidebar."
                : "mdwriter writes posts as Markdown in a GitHub repository and sends each one as its own pull request. It works in its own clone, so your checkout is never touched."}
            </p>
            {backup?.restorableSettings && (
              <div className="restore">
                <Cloud size={13} />
                <span className="grow">Your iCloud backup has projects and settings from another Mac.</span>
                <button type="button" className="btn small" onClick={() => run(api.restoreBackup)} disabled={busy}>
                  Restore
                </button>
              </div>
            )}
            {!onCancel &&
              archived.map((p) => (
                <div className="restore" key={p.id}>
                  <Archive size={13} />
                  <span className="grow">
                    {p.repo} <span className="muted">({p.branch}) is archived</span>
                  </span>
                  <button type="button" className="btn small" onClick={() => run(() => api.archiveProject(p.id, false))} disabled={busy}>
                    Unarchive
                  </button>
                </div>
              ))}
            <Field label="GitHub repository" hint="owner/name. Cloned over SSH, so your SSH key needs access.">
              <input
                autoFocus
                value={form.repo}
                placeholder="octocat/blog"
                onChange={(e) => set({ repo: e.target.value })}
                spellCheck={false}
              />
            </Field>
            <Field label="Branch" hint="Posts are read from this branch, and pull requests target it.">
              <input value={form.branch} onChange={(e) => set({ branch: e.target.value })} spellCheck={false} />
            </Field>
            <Field label="Posts folder" hint="Where posts live in the repository. The default fits most Astro sites.">
              <input value={form.postsDir} onChange={(e) => set({ postsDir: e.target.value })} spellCheck={false} />
            </Field>
            <div className="row settings-actions">
              <span className="grow error-text">{error}</span>
              {onCancel && (
                <button type="button" className="btn" onClick={onCancel} disabled={busy}>
                  Cancel
                </button>
              )}
              <button type="submit" className="btn primary" disabled={busy || !form.repo.trim()}>
                {busy && <LoaderCircle size={13} className="spin" />} {busy ? "Checking…" : "Continue"}
              </button>
            </div>
          </>
        ) : (
          <>
            <h1>Where do posts go live?</h1>
            <p className="muted">
              The Site button opens a published post at this URL. For an Astro project, mdwriter reads it from the
              project and checks it on the live site.
            </p>
            <div className="field">
              <span>Post URL</span>
              <div className="row input-row">
                <input
                  value={form.postUrl}
                  placeholder="https://example.com/blog/{slug}/"
                  onChange={(e) => {
                    set({ postUrl: e.target.value });
                    setDetected(null);
                  }}
                  spellCheck={false}
                  aria-label="Post URL"
                />
                <button type="button" className="btn" onClick={() => detect(form)} disabled={detecting}>
                  {detecting ? <LoaderCircle size={13} className="spin" /> : <Wand2 size={13} />} Detect site
                </button>
              </div>
              {detecting ? (
                <span className="hint">Cloning the repository and reading the Astro project…</span>
              ) : detected ? (
                <span className={`hint detect ${detected.verified ? "ok" : "warn-text"}`}>
                  {detected.verified ? <CircleCheck size={12} /> : <CircleAlert size={12} />}
                  <span>
                    From {detected.foundIn.join(", ")}. {detected.check}.
                  </span>
                </span>
              ) : (
                <span className="hint">{"{slug}"} is replaced by each post's slug.</span>
              )}
            </div>
            <div className="row settings-actions">
              <span className="grow error-text">{error}</span>
              <button type="button" className="btn" onClick={() => finish("")} disabled={busy}>
                Skip
              </button>
              <button type="submit" className="btn primary" disabled={busy || detecting || !form.postUrl.trim()}>
                {busy && <LoaderCircle size={13} className="spin" />} Start writing
              </button>
            </div>
          </>
        )}
      </form>
    </div>
  );
}

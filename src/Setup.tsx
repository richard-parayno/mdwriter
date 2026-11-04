import { useEffect, useState } from "react";
import { CircleAlert, CircleCheck, Cloud, LoaderCircle, Wand2 } from "lucide-react";
import { api, type BackupStatus, type Settings, type SiteDetection } from "./api";
import { Field } from "./SettingsPage";

/** First run: connect a repository, then find the live post URL. Shown until a repository is saved. */
export function Setup({ initial, onDone }: { initial: Settings; onDone: (s: Settings) => void }) {
  const [form, setForm] = useState(initial);
  const [step, setStep] = useState<"repo" | "site">("repo");
  const [busy, setBusy] = useState(false);
  const [detecting, setDetecting] = useState(false);
  const [error, setError] = useState("");
  const [detected, setDetected] = useState<SiteDetection | null>(null);
  const [backup, setBackup] = useState<BackupStatus | null>(null);
  const set = (patch: Partial<Settings>) => {
    setForm((f) => ({ ...f, ...patch }));
    setError("");
  };

  useEffect(() => {
    api.backupStatus().then(setBackup, () => {});
  }, []);

  async function detect(s: Settings) {
    setDetecting(true);
    setDetected(null);
    setError("");
    try {
      const d = await api.detectSite(s.branch, s.postsDir);
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
      // Checks the repository and branch are reachable before anything is saved.
      const saved = await api.saveSettings({ ...form, postUrl: "" });
      setForm(saved);
      setStep("site");
      void detect(saved);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function finish(postUrl: string) {
    setBusy(true);
    try {
      onDone(await api.saveSettings({ ...form, postUrl }));
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  }

  async function restore() {
    setBusy(true);
    try {
      onDone(await api.restoreBackup());
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
      >
        <div className="setup-steps">
          <span aria-current={step === "repo"}>1 · Repository</span>
          <span aria-current={step === "site"}>2 · Site</span>
        </div>

        {step === "repo" ? (
          <>
            <h1>Connect your site's repository</h1>
            <p className="muted">
              mdwriter writes posts as Markdown in a GitHub repository and sends each one as its own pull request. It
              works in its own clone, so your checkout is never touched.
            </p>
            {backup?.restorableSettings && (
              <div className="restore">
                <Cloud size={13} />
                <span className="grow">Your iCloud backup has settings from another Mac.</span>
                <button type="button" className="btn small" onClick={restore} disabled={busy}>
                  Restore
                </button>
              </div>
            )}
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

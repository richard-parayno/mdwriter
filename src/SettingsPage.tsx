import { useEffect, useState } from "react";
import { CircleCheck, CircleAlert, Cloud, LoaderCircle, Wand2 } from "lucide-react";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { api, type BackupStatus, type Settings, type SiteDetection } from "./api";

export function postUrl(settings: Settings | null, slug: string) {
  return settings?.postUrl ? settings.postUrl.replaceAll("{slug}", slug) : null;
}

function ago(ms: number) {
  const s = Math.round((Date.now() - ms) / 1000);
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.round(s / 60)} min ago`;
  return new Date(ms).toLocaleString();
}

export function SettingsPage({
  settings,
  onSaved,
  onClose,
}: {
  settings: Settings;
  onSaved: (s: Settings) => void;
  onClose: () => void;
}) {
  const [form, setForm] = useState(settings);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const [detecting, setDetecting] = useState(false);
  const [detected, setDetected] = useState<SiteDetection | null>(null);
  const [detectError, setDetectError] = useState("");
  const [backup, setBackup] = useState<BackupStatus | null>(null);
  const [restoring, setRestoring] = useState(false);
  const set = (patch: Partial<Settings>) => {
    setForm((f) => ({ ...f, ...patch }));
    setError("");
  };
  const repoChanged = form.repo.trim().replace(/\.git$/, "") !== settings.repo;

  useEffect(() => {
    api.backupStatus().then(setBackup, (e) => setError(String(e)));
  }, []);

  async function save() {
    setSaving(true);
    try {
      onSaved(await api.saveSettings(form));
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  }

  async function detect() {
    setDetecting(true);
    setDetected(null);
    setDetectError("");
    try {
      const d = await api.detectSite(form.branch, form.postsDir);
      setDetected(d);
      set({ postUrl: d.url });
    } catch (e) {
      setDetectError(String(e));
    } finally {
      setDetecting(false);
    }
  }

  async function restore() {
    setRestoring(true);
    try {
      onSaved(await api.restoreBackup());
    } catch (e) {
      setError(String(e));
      api.backupStatus().then(setBackup);
    } finally {
      setRestoring(false);
    }
  }

  const restorable = backup && (backup.restorableDrafts.length > 0 || backup.restorableSettings);

  return (
    <form
      className="settings"
      onSubmit={(e) => {
        e.preventDefault();
        save();
      }}
    >
      <h2>Repository</h2>
      <Field label="GitHub repository" hint="owner/name. mdwriter clones it over SSH into its own folder.">
        <input value={form.repo} onChange={(e) => set({ repo: e.target.value })} spellCheck={false} />
      </Field>
      {repoChanged && (
        <div className="hint warn-text">Saving re-clones the new repository. Your local drafts stay.</div>
      )}
      <Field label="Branch" hint="Posts are read from this branch, and new PRs target it.">
        <input value={form.branch} onChange={(e) => set({ branch: e.target.value })} spellCheck={false} />
      </Field>
      <Field label="Posts folder" hint="Where posts live in the repository. Each post is published to <folder>/<slug>/.">
        <input value={form.postsDir} onChange={(e) => set({ postsDir: e.target.value })} spellCheck={false} />
      </Field>

      <h2>Site</h2>
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
          <button type="button" className="btn" onClick={detect} disabled={detecting} title="Read the Astro project on the branch and check the URL on the live site">
            {detecting ? <LoaderCircle size={13} className="spin" /> : <Wand2 size={13} />} Detect site
          </button>
        </div>
        {detected ? (
          <span className={`hint detect ${detected.verified ? "ok" : "warn-text"}`}>
            {detected.verified ? <CircleCheck size={12} /> : <CircleAlert size={12} />}
            <span>
              From {detected.foundIn.join(", ")}. {detected.check}.
              {!detected.verified && " Check the URL before saving."}
            </span>
          </span>
        ) : detectError ? (
          <span className="hint error-text">{detectError}</span>
        ) : (
          <span className="hint">
            The live URL of a post; {"{slug}"} is replaced by its slug. Leave empty to hide the Site button.
          </span>
        )}
      </div>

      <h2>Editor</h2>
      <Field label="Font size" hint="In pixels, 10–24.">
        <input
          type="number"
          min={10}
          max={24}
          className="num"
          value={form.editorFontSize}
          onChange={(e) => set({ editorFontSize: e.target.valueAsNumber || 0 })}
        />
      </Field>
      <Toggle
        label="Live preview"
        hint="Hide markdown symbols except on the line you're editing, style headings and links, and show images inline. The file stays plain markdown."
        checked={form.livePreview}
        onChange={(livePreview) => set({ livePreview })}
      />

      <Toggle
        label="Line numbers"
        hint="Number the lines of the post body, in the editor and in diffs."
        checked={form.lineNumbers}
        onChange={(lineNumbers) => set({ lineNumbers })}
      />

      <h2>Backup</h2>
      <Toggle
        label="Back up to iCloud Drive"
        hint="Copies every draft (with its images) and these settings to iCloud Drive/mdwriter as they save. Your other Macs can restore from it. Nothing in the backup is ever deleted."
        checked={form.backup}
        disabled={!backup?.dir}
        onChange={(b) => set({ backup: b })}
      />
      {backup && !backup.dir && <div className="hint warn-text">iCloud Drive is off on this Mac. Turn it on in System Settings › Apple Account › iCloud.</div>}
      {backup?.dir && settings.backup && (
        <div className="hint backup-status">
          <Cloud size={12} />
          {backup.error ? (
            <span className="error-text">Last backup failed: {backup.error}</span>
          ) : (
            <span>{backup.lastBackup ? `Backed up ${ago(backup.lastBackup)}` : "On. Drafts back up as they save."}</span>
          )}
          <button type="button" className="link-btn" onClick={() => revealItemInDir(backup.dir!).catch((e) => setError(String(e)))}>
            Show in Finder
          </button>
        </div>
      )}
      {restorable && (
        <div className="restore">
          <span className="grow">
            The iCloud backup has{" "}
            {[
              backup.restorableDrafts.length > 0 &&
                `${backup.restorableDrafts.length} draft${backup.restorableDrafts.length === 1 ? "" : "s"} not on this Mac`,
              backup.restorableSettings && "different settings",
            ]
              .filter(Boolean)
              .join(" and ")}
            . Restoring keeps every draft already here.
          </span>
          <button type="button" className="btn small" onClick={restore} disabled={restoring}>
            {restoring && <LoaderCircle size={12} className="spin" />} Restore
          </button>
        </div>
      )}

      <div className="row settings-actions">
        <span className="grow error-text">{error}</span>
        <button type="button" className="btn" onClick={onClose}>
          Cancel
        </button>
        <button type="submit" className="btn primary" disabled={saving}>
          {saving && <LoaderCircle size={13} className="spin" />} Save
        </button>
      </div>
    </form>
  );
}

function Field({ label, hint, children }: { label: string; hint: string; children: React.ReactNode }) {
  return (
    <label className="field">
      <span>{label}</span>
      {children}
      <span className="hint">{hint}</span>
    </label>
  );
}

function Toggle({
  label,
  hint,
  checked,
  disabled,
  onChange,
}: {
  label: string;
  hint: string;
  checked: boolean;
  disabled?: boolean;
  onChange: (v: boolean) => void;
}) {
  return (
    <label className="toggle">
      <input type="checkbox" role="switch" checked={checked} disabled={disabled} onChange={(e) => onChange(e.target.checked)} />
      <span>
        <span className="toggle-label">{label}</span>
        <span className="hint">{hint}</span>
      </span>
    </label>
  );
}

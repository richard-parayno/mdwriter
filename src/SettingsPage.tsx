import { useEffect, useMemo, useState } from "react";
import { Archive, CircleCheck, CircleAlert, Cloud, LoaderCircle, RotateCcw, Wand2 } from "lucide-react";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { activeProject, api, type BackupStatus, type Project, type Settings, type SiteDetection } from "./api";
import { ACTIONS, bindings, display, fromEvent, problem, recording, type ActionId } from "./keymap";

export function postUrl(project: Project | null, slug: string) {
  return project?.postUrl ? project.postUrl.replaceAll("{slug}", slug) : null;
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
  const active = activeProject(settings)!;
  const [proj, setProj] = useState(active);
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
  const setP = (patch: Partial<Project>) => {
    setProj((p) => ({ ...p, ...patch }));
    setError("");
  };
  const repoChanged = proj.repo.trim().replace(/\.git$/, "") !== active.repo;
  const archived = settings.projects.filter((p) => p.archived);

  useEffect(() => {
    api.backupStatus().then(setBackup, (e) => setError(String(e)));
  }, []);

  async function save() {
    setSaving(true);
    try {
      onSaved(await api.saveSettings({ ...form, projects: form.projects.map((p) => (p.id === proj.id ? proj : p)) }));
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
      const d = await api.detectSite(proj.branch, proj.postsDir);
      setDetected(d);
      setP({ postUrl: d.url });
    } catch (e) {
      setDetectError(String(e));
    } finally {
      setDetecting(false);
    }
  }

  /** Archives or unarchives a project at once, apart from the form. */
  async function archive(id: string, value: boolean) {
    try {
      onSaved(await api.archiveProject(id, value));
    } catch (e) {
      setError(String(e));
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
      <div className="row section-head">
        <h2 className="grow">Project</h2>
        <button
          type="button"
          className="btn small"
          title="Hide this project from the switcher. Its clone and drafts stay on this Mac, and you can unarchive it here."
          onClick={() => archive(active.id, true)}
        >
          <Archive size={12} /> Archive
        </button>
      </div>
      <Field label="GitHub repository" hint="owner/name. mdwriter clones it over SSH into its own folder.">
        <input value={proj.repo} onChange={(e) => setP({ repo: e.target.value })} spellCheck={false} />
      </Field>
      {repoChanged && (
        <div className="hint warn-text">Saving re-clones the new repository. This project's drafts stay.</div>
      )}
      <Field label="Branch" hint="Posts are read from this branch, and new PRs target it.">
        <input value={proj.branch} onChange={(e) => setP({ branch: e.target.value })} spellCheck={false} />
      </Field>
      <Field label="Posts folder" hint="Where posts live in the repository. Each post is published to <folder>/<slug>/.">
        <input value={proj.postsDir} onChange={(e) => setP({ postsDir: e.target.value })} spellCheck={false} />
      </Field>

      <h2>Site</h2>
      <div className="field">
        <span>Post URL</span>
        <div className="row input-row">
          <input
            value={proj.postUrl}
            placeholder="https://example.com/blog/{slug}/"
            onChange={(e) => {
              setP({ postUrl: e.target.value });
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

      <h2>Keyboard shortcuts</h2>
      <Keybinds value={form.keybinds} onChange={(keybinds) => set({ keybinds })} />

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

      {archived.length > 0 && (
        <>
          <h2>Archived projects</h2>
          {archived.map((p) => (
            <div className="row archived" key={p.id}>
              <span className="grow">
                {p.repo} <span className="muted">· {p.branch}</span>
              </span>
              <button type="button" className="btn small" onClick={() => archive(p.id, false)}>
                Unarchive
              </button>
            </div>
          ))}
          <div className="hint">Archived projects keep their clone and drafts on this Mac.</div>
        </>
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

export function Field({ label, hint, children }: { label: string; hint: string; children: React.ReactNode }) {
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

/** Every action with its shortcut. Click one and press keys to change it; Backspace clears, Escape cancels. */
function Keybinds({ value, onChange }: { value: Record<string, string>; onChange: (v: Record<string, string>) => void }) {
  const [editing, setEditing] = useState<ActionId | null>(null);
  const [note, setNote] = useState("");
  const keys = useMemo(() => bindings({ keybinds: value } as Settings), [value]);

  useEffect(() => {
    if (!editing) return;
    recording.current = true;
    const set = (binding: string) => {
      const next = { ...value };
      if (binding === ACTIONS.find((a) => a.id === editing)!.key) delete next[editing];
      else next[editing] = binding;
      onChange(next);
      setEditing(null);
    };
    const onKey = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      const bare = !e.metaKey && !e.ctrlKey && !e.altKey && !e.shiftKey;
      if (bare && e.key === "Escape") return setEditing(null);
      if (bare && (e.key === "Backspace" || e.key === "Delete")) return set("");
      const b = fromEvent(e);
      if (!b) return;
      const why = problem(b);
      if (why) return setNote(why);
      const taken = ACTIONS.find((a) => a.id !== editing && keys[a.id] === b);
      if (taken) return setNote(`${display(b)} is already ${taken.label}`);
      set(b);
    };
    window.addEventListener("keydown", onKey, true);
    return () => {
      recording.current = false;
      window.removeEventListener("keydown", onKey, true);
    };
  }, [editing, value, onChange, keys]);

  return (
    <div className="keybinds">
      {ACTIONS.map((a) => {
        const changed = a.id in value;
        return (
          <div className="row keybind" key={a.id}>
            <span className="grow">{a.label}</span>
            {changed && (
              <button
                type="button"
                className="icon-btn"
                title={a.key ? `Reset to ${display(a.key)}` : "Reset to none"}
                onClick={() => {
                  const next = { ...value };
                  delete next[a.id];
                  onChange(next);
                }}
              >
                <RotateCcw size={11} />
              </button>
            )}
            <button
              type="button"
              className="kbd-btn"
              aria-pressed={editing === a.id}
              onClick={() => {
                setNote("");
                setEditing(editing === a.id ? null : a.id);
              }}
              onBlur={() => editing === a.id && setEditing(null)}
            >
              {editing === a.id ? "Press keys…" : keys[a.id] ? <kbd>{display(keys[a.id])}</kbd> : <span className="muted">None</span>}
            </button>
          </div>
        );
      })}
      <span className={`hint ${note ? "warn-text" : ""}`}>
        {note || "Click a shortcut and press new keys. Backspace removes it, Escape cancels."}
      </span>
    </div>
  );
}

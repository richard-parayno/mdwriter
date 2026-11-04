import { invoke } from "@tauri-apps/api/core";

export type PrInfo = { number: number; url: string; state: "OPEN" | "MERGED" | "CLOSED"; headOid: string };
/** Sync state of a local draft against the branch it tracks. */
export type SyncStatus = {
  /** The main branch or `post/<slug>`. */
  upstream: string;
  baseKnown: boolean;
  /** Lines changed in index.md plus other files changed since the last sync. */
  changes: number;
  /** Commits on the upstream that changed the post since the last sync. */
  behind: number;
  onRemote: boolean;
  inSync: boolean;
};
export type PostSummary = {
  slug: string;
  local: boolean;
  content: string;
  sync: SyncStatus | null;
  pr: PrInfo | null;
  onMain: boolean;
  conflict: boolean;
};
export type PostList = { posts: PostSummary[]; warnings: string[]; syncedAt: number | null };
export type PostFiles = { dir: string; images: string[] };
export type TextConflict = { name: string; ours: string; theirs: string };
export type Conflict = { upstream: string; text: TextConflict | null; binary: string[]; remoteDir: string };
export type PullOutcome = { kind: "upToDate" } | { kind: "updated"; files: string[] } | ({ kind: "conflict" } & Omit<Conflict, "remoteDir">);
export type Commit = { sha: string; date: number; subject: string };
/** One repository mdwriter writes to. */
export type Project = {
  /** Folder name in the app data dir; assigned by the backend when the project is added. */
  id: string;
  /** `owner/name` on GitHub. */
  repo: string;
  branch: string;
  postsDir: string;
  /** Live URL of a post with `{slug}` in place of the slug; empty hides the Site button. */
  postUrl: string;
  archived: boolean;
};
export type Settings = {
  editorFontSize: number;
  livePreview: boolean;
  lineNumbers: boolean;
  backup: boolean;
  /** Shortcut overrides by action id; "" turns one off. Defaults are in keymap.ts. */
  keybinds: Record<string, string>;
  projects: Project[];
  activeProject: string;
};
export const emptyProject: Project = { id: "", repo: "", branch: "main", postsDir: "src/content/posts", postUrl: "", archived: false };

export function activeProject(s: Settings | null) {
  return s?.projects.find((p) => p.id === s.activeProject && !p.archived) ?? null;
}

export type BackupStatus = {
  dir: string | null;
  lastBackup: number | null;
  error: string | null;
  restorableDrafts: string[];
  restorableSettings: boolean;
};
export type SiteDetection = { url: string; foundIn: string[]; check: string; verified: boolean };

// The project post commands act on. Read when each call is made, so switch only after pending saves are flushed.
let project = "";
export function setApiProject(id: string) {
  project = id;
}
const call = <T>(cmd: string, args: Record<string, unknown> = {}) => invoke<T>(cmd, { project, ...args });

export const api = {
  listPosts: (fetch: boolean) => call<PostList>("list_posts", { fetch }),
  createDraft: (slug: string, content: string) => call<void>("create_draft", { slug, content }),
  readDraft: (slug: string) => call<string>("read_draft", { slug }),
  saveDraft: (slug: string, content: string) => call<void>("save_draft", { slug, content }),
  postFiles: (slug: string) => call<PostFiles>("post_files", { slug }),
  addImage: (slug: string, source: string) => call<string>("add_image", { slug, source }),
  pull: (slug: string) => call<PullOutcome>("pull_post", { slug }),
  push: (slug: string) => call<string>("push_post", { slug }),
  openPr: (slug: string, title: string, body: string) => call<PrInfo>("open_pr", { slug, title, body }),
  conflict: (slug: string) => call<Conflict | null>("conflict_state", { slug }),
  resolve: (slug: string, text: string | null, remoteImages: string[]) => call<void>("resolve_conflict", { slug, text, remoteImages }),
  abortConflict: (slug: string) => call<void>("abort_conflict", { slug }),
  undoPull: (slug: string) => call<void>("undo_pull", { slug }),
  edit: (slug: string) => call<void>("edit_post", { slug }),
  history: (slug: string) => call<Commit[]>("post_history", { slug }),
  /** index.md at "base" (last synced), "main", or a commit hash. */
  textAt: (slug: string, rev: string) => call<string | null>("text_at", { slug, rev }),
  /** `id` defaults to the current project; setup passes the project it just added. */
  detectSite: (branch: string, postsDir: string, id = project) => invoke<SiteDetection>("detect_site", { project: id, branch, postsDir }),
  getSettings: () => invoke<Settings>("get_settings"),
  saveSettings: (settings: Settings) => invoke<Settings>("save_settings", { settings }),
  /** Checks the repository, adds it, and makes it the active project. */
  addProject: (project: Project) => invoke<Settings>("add_project", { project }),
  switchProject: (id: string) => invoke<Settings>("switch_project", { id }),
  archiveProject: (id: string, archived: boolean) => invoke<Settings>("archive_project", { id, archived }),
  backupStatus: () => invoke<BackupStatus>("backup_status"),
  restoreBackup: () => invoke<Settings>("restore_backup"),
};

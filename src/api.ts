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
export type Settings = {
  repo: string;
  branch: string;
  postsDir: string;
  /** Live URL of a post with `{slug}` in place of the slug; empty hides the Site button. */
  postUrl: string;
  editorFontSize: number;
  livePreview: boolean;
  lineNumbers: boolean;
  backup: boolean;
};
export type BackupStatus = {
  dir: string | null;
  lastBackup: number | null;
  error: string | null;
  restorableDrafts: string[];
  restorableSettings: boolean;
};
export type SiteDetection = { url: string; foundIn: string[]; check: string; verified: boolean };

export const api = {
  listPosts: (fetch: boolean) => invoke<PostList>("list_posts", { fetch }),
  createDraft: (slug: string, content: string) => invoke<void>("create_draft", { slug, content }),
  readDraft: (slug: string) => invoke<string>("read_draft", { slug }),
  saveDraft: (slug: string, content: string) => invoke<void>("save_draft", { slug, content }),
  postFiles: (slug: string) => invoke<PostFiles>("post_files", { slug }),
  addImage: (slug: string, source: string) => invoke<string>("add_image", { slug, source }),
  pull: (slug: string) => invoke<PullOutcome>("pull_post", { slug }),
  push: (slug: string) => invoke<string>("push_post", { slug }),
  openPr: (slug: string, title: string, body: string) => invoke<PrInfo>("open_pr", { slug, title, body }),
  conflict: (slug: string) => invoke<Conflict | null>("conflict_state", { slug }),
  resolve: (slug: string, text: string | null, remoteImages: string[]) =>
    invoke<void>("resolve_conflict", { slug, text, remoteImages }),
  abortConflict: (slug: string) => invoke<void>("abort_conflict", { slug }),
  undoPull: (slug: string) => invoke<void>("undo_pull", { slug }),
  edit: (slug: string) => invoke<void>("edit_post", { slug }),
  history: (slug: string) => invoke<Commit[]>("post_history", { slug }),
  /** index.md at "base" (last synced), "main", or a commit hash. */
  textAt: (slug: string, rev: string) => invoke<string | null>("text_at", { slug, rev }),
  getSettings: () => invoke<Settings>("get_settings"),
  saveSettings: (settings: Settings) => invoke<Settings>("save_settings", { settings }),
  backupStatus: () => invoke<BackupStatus>("backup_status"),
  restoreBackup: () => invoke<Settings>("restore_backup"),
  detectSite: (branch: string, postsDir: string) => invoke<SiteDetection>("detect_site", { branch, postsDir }),
};

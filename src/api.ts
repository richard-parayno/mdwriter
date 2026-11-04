import { invoke } from "@tauri-apps/api/core";

export type Status = "draft" | "review" | "published" | "changed";
export type Pr = { number: number; url: string };
export type PostSummary = {
  slug: string;
  status: Status;
  pr: Pr | null;
  local: boolean;
  dirty: boolean;
  content: string;
};
export type PostList = { posts: PostSummary[]; warnings: string[] };
export type PostFiles = { dir: string; images: string[] };
export type PublishResult = { number: number; url: string; created: boolean; pushed: boolean };
export type StepId = "sync" | "branch" | "commit" | "push" | "pr";
export type Progress = { step: StepId; state: "active" | "done" | "error"; detail: string };
export type Settings = {
  repo: string;
  branch: string;
  postsDir: string;
  /** Live URL of a post with `{slug}` in place of the slug; empty hides the Site button. */
  postUrl: string;
  editorFontSize: number;
  livePreview: boolean;
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
  publish: (slug: string, title: string, body: string) =>
    invoke<PublishResult>("publish", { slug, title, body }),
  getSettings: () => invoke<Settings>("get_settings"),
  saveSettings: (settings: Settings) => invoke<Settings>("save_settings", { settings }),
  backupStatus: () => invoke<BackupStatus>("backup_status"),
  restoreBackup: () => invoke<Settings>("restore_backup"),
  detectSite: (branch: string, postsDir: string) => invoke<SiteDetection>("detect_site", { branch, postsDir }),
};

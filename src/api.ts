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

export const api = {
  listPosts: (fetch: boolean) => invoke<PostList>("list_posts", { fetch }),
  createDraft: (slug: string, content: string) => invoke<void>("create_draft", { slug, content }),
  readDraft: (slug: string) => invoke<string>("read_draft", { slug }),
  saveDraft: (slug: string, content: string) => invoke<void>("save_draft", { slug, content }),
  postFiles: (slug: string) => invoke<PostFiles>("post_files", { slug }),
  addImage: (slug: string, source: string) => invoke<string>("add_image", { slug, source }),
  publish: (slug: string, title: string, body: string) =>
    invoke<PublishResult>("publish", { slug, title, body }),
};

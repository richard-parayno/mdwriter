import { parse, stringify } from "yaml";

/** Mirrors the `posts` collection schema in the blog's src/content.config.ts. */
export type Frontmatter = {
  title: string;
  description: string;
  pubDate: string; // YYYY-MM-DD
  tags: string[];
  draft: boolean;
};

export type Post = { fm: Frontmatter; body: string; extra: Record<string, unknown> };

const FENCE = /^---\r?\n([\s\S]*?)\r?\n---\r?\n?/;

function toDate(value: unknown): string {
  if (value instanceof Date) return value.toISOString().slice(0, 10);
  const s = String(value ?? "");
  return /^\d{4}-\d{2}-\d{2}/.test(s) ? s.slice(0, 10) : today();
}

export function today(): string {
  const d = new Date();
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

export function parsePost(raw: string): Post {
  const match = raw.match(FENCE);
  const data = (match ? parse(match[1]) : {}) ?? {};
  const { title, description, pubDate, tags, draft, ...extra } = data as Record<string, unknown>;
  const body = match ? raw.slice(match[0].length).replace(/^\r?\n/, "") : raw;
  return {
    fm: {
      title: String(title ?? ""),
      description: String(description ?? ""),
      pubDate: toDate(pubDate),
      tags: Array.isArray(tags) ? tags.map(String) : [],
      draft: draft === true,
    },
    body,
    extra,
  };
}

export function serializePost({ fm, body, extra }: Post): string {
  const q = (s: string) => JSON.stringify(s);
  const lines = [
    "---",
    `title: ${q(fm.title)}`,
    `description: ${q(fm.description)}`,
    `pubDate: ${fm.pubDate}`,
    fm.tags.length ? `tags:\n${fm.tags.map((t) => `  - ${q(t)}`).join("\n")}` : "tags: []",
    `draft: ${fm.draft}`,
  ];
  if (Object.keys(extra).length) lines.push(stringify(extra).trimEnd());
  lines.push("---", "", body);
  return lines.join("\n");
}

export function slugify(title: string): string {
  return title
    .toLowerCase()
    .normalize("NFKD")
    .replace(/[\u0300-\u036f]/g, "")
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 60)
    .replace(/-+$/, "");
}

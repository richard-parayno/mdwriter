# mdwriter

A personal macOS app (Tauri v2 + React) for writing posts for my Astro blog ([richard-parayno/blog](https://github.com/richard-parayno/blog)) and publishing each post as its own GitHub PR.

The blog repo is set in `src-tauri/src/lib.rs` (`REMOTE`, `GH_REPO`, `POSTS_DIR`). The frontmatter editor follows the blog's `posts` schema in `src/content.config.ts`.

## Run

Needs Rust, the Tauri CLI (`cargo install tauri-cli`), pnpm, `git` with SSH access to GitHub, and `gh` logged in.

```sh
pnpm install
cargo tauri dev      # development, with hot reload
cargo tauri build    # builds src-tauri/target/release/bundle/macos/Writer.app
```

## How it works

- **Drafts** live in `~/Library/Application Support/com.richardparayno.blogwriter/drafts/<slug>/index.md`, with dropped images copied beside them and referenced as `./name.png`. They save automatically as you type.
- **Git work** happens in the app's own clone at `.../com.richardparayno.blogwriter/repo`, never in your own checkout of the blog.
- **Publish as PR** fetches `origin`, creates `post/<slug>` from `origin/main`, copies the draft folder to `src/content/posts/<slug>/`, commits only that folder (`post: <slug>`), pushes, and runs `gh pr create`. If `post/<slug>` already has an open PR, the app pushes a new commit to that branch instead.
- **Status** comes from `gh pr list` (open PRs by head branch) and `origin/main`:
  - Local draft: not on main and has no PR.
  - In review: has an open PR.
  - Published: on main and identical to the draft.
  - Merged · draft: on main with `draft: true`.
  - Unpublished changes: on main, and the local draft differs.
- Posts that exist only on `main` (including the older single-file posts) are shown read-only.

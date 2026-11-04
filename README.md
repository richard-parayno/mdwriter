# mdwriter

A macOS writing app for Markdown posts that live in a git repository. You write in a focused editor, and each post goes to the repository as its own branch and pull request. Nothing is published until that PR is merged.

It's built for people who already run their site from a git repository, with a branch that deploys and pull requests as the way changes get in. [Astro](https://astro.build) content collections are a first-class target, but any repository that keeps posts as Markdown files in one folder works.

Built with Tauri v2, React and CodeMirror.

## Architecture

Everything you write stays on your Mac until you choose an action. The app works in its own clone of your repository, never in your checkout, and only talks to the remote through `git` and the GitHub CLI.

```text
 YOUR MAC
┌──────────────────────────────────────────────────────────────────────────┐
│ mdwriter.app                                                             │
│                                                                          │
│   Webview: React + CodeMirror                                            │
│   editor, live preview, diffs, change markers, command palette           │
│          │                                         ▲                     │
│          │ Tauri commands                          │ results             │
│          ▼                                         │                     │
│   Rust backend: src-tauri/src/lib.rs, git logic in sync.rs               │
│                                                                          │
└───────┬───────────────────────────┬────────────────────────┬─────────────┘
        │ read and write            │ git, gh                │ copy, if on
        ▼                           ▼                        ▼
┌──────────────────────┐  ┌──────────────────────┐  ┌──────────────────────┐
│ App data folder      │  │ App-owned clone      │  │ iCloud Drive         │
│                      │  │                      │  │                      │
│ drafts/<slug>/       │  │ repo/                │  │ mdwriter/            │
│   index.md, images   │  │ commits only the     │  │ drafts and settings, │
│   .base/ (last sync) │  │ post's folder        │  │ restorable on        │
│ settings.json        │  │                      │  │ another Mac          │
│ sync.json (PR cache) │  │                      │  │                      │
└──────────────────────┘  └───────────┬──────────┘  └──────────────────────┘
                                      │
══════════════════════════════════════│══════════════════════════ network ══
 REMOTE                               │
             git fetch: reads only    │    git push --force-with-lease
             gh pr list, gh pr create │    to post/<slug>
                                      ▼
┌──────────────────────────────────────────────────────────────────────────┐
│ GitHub repository                                                        │
│                                                                          │
│   post/<slug>  ── pull request ──▶  main (or your configured branch)     │
└───────────────────────────────────────────────────┬──────────────────────┘
                                                    │ your host builds the
                                                    ▼ branch and deploys it
                                         ┌──────────────────────┐
                                         │ Live site            │
                                         │ opened by the Site   │
                                         │ button               │
                                         └──────────────────────┘
```

Three rules keep local and remote from overwriting each other:

- **Reads are automatic, writes are explicit.** Opening the app lists posts from local state at once, then a background `git fetch` and `gh pr list` refresh the counts. Drafts and the remote only change when you pick Pull, Push or Open PR.
- **Upstream is the source of truth.** Each draft remembers the remote version it last matched (`.base/`). Push refuses while the remote has changes you haven't pulled, and pushes with `--force-with-lease`, so GitHub also refuses it if the branch moved after the last fetch.
- **Pull never overwrites your work.** Remote changes merge into the draft. If they clash with yours, the draft stays untouched until you resolve the conflict in a side-by-side view.

## Requirements

- macOS
- Rust, the Tauri CLI (`cargo install tauri-cli`) and pnpm, to build it
- `git` with SSH access to your repository on GitHub
- The GitHub CLI, `gh`, logged in

## Run

```sh
pnpm install
cargo tauri dev      # development, with hot reload
cargo tauri build    # builds src-tauri/target/release/bundle/macos/mdwriter.app
pnpm release         # builds, installs to /Applications, and restarts mdwriter if it was open
```

## Setup

Open Settings (⌘,) and set:

- **GitHub repository**, as `owner/name`. mdwriter clones it into its own folder.
- **Branch**: the branch posts are read from and pull requests target, usually `main`.
- **Posts folder**: where posts live in the repository, such as `src/content/posts`.
- **Post URL**: the live URL of a post, with `{slug}` in place of the slug. For an Astro project, **Detect site** works it out for you (see below).

Settings are saved in `~/Library/Application Support/com.richardparayno.mdwriter/settings.json`.

## Astro support

- **Detect site** reads `site`, `base`, `trailingSlash` and `build.format` from `astro.config`, finds the collection that loads your posts folder in the content config, and finds the page in `src/pages` that renders it. It then requests one published post to check the URL.
- **Post IDs** match Astro's glob loader: `posts/foo/index.md` and `posts/foo.md` are both the post `foo`.
- **Frontmatter** gets its own fields for `title`, `description`, `pubDate`, `tags` and `draft`. Other frontmatter keys are kept as they are.
- **`draft: true`** posts are labelled Hidden, and the Site button is hidden for them, since Astro sites usually leave drafts out of the build.

Outside Astro, set the Post URL by hand. The frontmatter fields above are still what the editor shows.

## How it works

- **Drafts** live in `~/Library/Application Support/com.richardparayno.mdwriter/drafts/<slug>/index.md`, with dropped images copied beside them and referenced as `./name.png`. They save automatically as you type, through a temporary file so a crash can't truncate them, and ⌘Q saves any pending edit before quitting.
- **Each post is a branch, `post/<slug>`.** A draft keeps a hidden `.base/` copy of the files as they last matched the remote, plus `.mdwriter.json` with that commit.
- **Pull** (⌘⇧L) fetches, then merges remote changes into the draft. Without clashes, they merge in and **Undo** restores the exact draft. With clashes, a side-by-side view opens: the left side (yours, with the clean remote changes already merged in) becomes the draft on **Resolve**. **Abort** drops the pull.
- **Push** (⌘⇧P) commits only the post's folder to `post/<slug>` and pushes it. Single-file posts stay single files until they get images.
- **Open PR** opens a pull request from `post/<slug>` into your branch, as its own step. Once the PR is merged, the draft tracks your branch again.
- **Edit** starts a draft from a post that's already on your branch.
- **Review changes** shows the remote against your draft: what Push would send. **Change markers** (⌘⇧D, or the picker in the footer) mark lines added, changed or removed since the last sync, since your branch, or since any commit of the post. Click a marker to see the old text and revert it.
- **Backup**, when on, copies each draft and the settings to `iCloud Drive/mdwriter` as they save. It never deletes anything there.
- **⌘K** lists every action and post.

The git logic is in `src-tauri/src/sync.rs`. Its tests run each guarantee against a local bare repository: `cargo test --lib` in `src-tauri/`.

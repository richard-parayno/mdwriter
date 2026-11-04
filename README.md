# mdwriter

A personal macOS app (Tauri v2 + React) for writing posts for my Astro blog ([richard-parayno/blog](https://github.com/richard-parayno/blog)) and publishing each post as its own GitHub PR.

The repository, branch (`main` by default), posts folder, live post URL (Detect site reads it from the Astro project and checks it on the live site), editor font size, live preview, and iCloud backup are set on the Settings page (gear icon) and saved to `.../com.richardparayno.mdwriter/settings.json`. Below, `main` means the configured branch. The frontmatter editor follows the blog's `posts` schema in `src/content.config.ts`.

## Run

Needs Rust, the Tauri CLI (`cargo install tauri-cli`), pnpm, `git` with SSH access to GitHub, and `gh` logged in.

```sh
pnpm install
cargo tauri dev      # development, with hot reload
cargo tauri build    # builds src-tauri/target/release/bundle/macos/mdwriter.app
```

## How it works

- **Drafts** live in `~/Library/Application Support/com.richardparayno.mdwriter/drafts/<slug>/index.md`, with dropped images copied beside them and referenced as `./name.png`. They save automatically as you type.
- **Backup**, when on, copies each draft and `settings.json` to `iCloud Drive/mdwriter` as they save. It never deletes anything there. On another Mac, Settings offers to restore drafts that aren't on that Mac and the backed-up settings.
- **Git work** happens in the app's own clone at `.../com.richardparayno.mdwriter/repo`, never in your own checkout of the blog.
- **Each post is a branch, `post/<slug>`.** Upstream is the source of truth. Each draft keeps a hidden `.base/` copy of the files as they were the last time it matched the remote, plus `.mdwriter.json` with the commit. The git logic is in `src-tauri/src/sync.rs`.
- **Opening the app only reads.** Posts list straight away from local state, and a background fetch plus `gh pr list` updates the counts. Nothing changes a draft or GitHub until you pick an action.
- **Pull** (⌘⇧L) fetches, then merges remote changes into the draft. With no clashes, they merge in and **Undo** restores the exact draft. With clashes, the draft stays untouched and a side-by-side view opens: the left side (yours, with the clean remote changes already merged in) becomes the draft on **Resolve**. **Abort** drops the pull.
- **Push** (⌘⇧P) commits the draft to `post/<slug>` and pushes. It refuses while the remote has changes you haven't pulled, and the push carries `--force-with-lease`, so GitHub also refuses it if the branch moved since the last fetch. Single-file posts stay single files until they get images.
- **Open PR** opens a PR from `post/<slug>` into the main branch, as its own step. Once the PR is merged, the draft tracks the main branch again.
- **Edit** starts a draft from a post that is only on the main branch.
- **Review changes** shows the remote against your draft: what Push would send. **Change markers** (⌘⇧D, or the picker in the footer) mark lines added, changed or removed since the last sync, the main branch, or any commit of the post. Click a marker to see the old text and revert it.
- **⌘K** lists every action and post.

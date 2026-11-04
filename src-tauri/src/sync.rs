//! Git sync for drafts. Nothing here knows about Tauri, so the tests can run it against a local bare repository.
//!
//! A draft folder holds the post's files plus two hidden entries:
//! - `.base/`: the files as they were the last time the draft matched the remote. Pull merges against it, and
//!   Push refuses while the remote no longer matches it. No `.base/` means the base is unknown, so every
//!   difference from the remote is treated as a conflict.
//! - `.mdwriter.json`: the branch and commit the base came from.
//!
//! Upstream is the source of truth: Push only succeeds when the remote still matches the base, and the push
//! itself carries a lease, so GitHub refuses it if the branch moved after the last fetch.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// File name -> git blob hash.
pub type Files = BTreeMap<String, String>;

pub const META: &str = ".mdwriter.json";
pub const BASE: &str = ".base";

// ---------- process and file helpers ----------

/// GUI apps launched from Finder get a minimal PATH; make sure git and gh resolve.
pub fn command(program: &str, cwd: &Path) -> Command {
    let mut cmd = Command::new(program);
    let path = std::env::var("PATH").unwrap_or_default();
    cmd.current_dir(cwd)
        .env("PATH", format!("/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:{path}"))
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GH_PROMPT_DISABLED", "1");
    cmd
}

fn run(cwd: &Path, program: &str, args: &[&str]) -> Result<std::process::Output, String> {
    command(program, cwd).args(args).output().map_err(|e| format!("Could not run {program}: {e}"))
}

pub fn exec(cwd: &Path, program: &str, args: &[&str]) -> Result<String, String> {
    let out = run(cwd, program, args)?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
    } else {
        Err(format!("`{program} {}` failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()))
    }
}

pub fn git(repo: &Path, args: &[&str]) -> Result<String, String> {
    exec(repo, "git", args)
}

fn blob(repo: &Path, hash: &str) -> Result<Vec<u8>, String> {
    let out = run(repo, "git", &["cat-file", "blob", hash])?;
    if out.status.success() {
        Ok(out.stdout)
    } else {
        Err(format!("Could not read blob {hash}: {}", String::from_utf8_lossy(&out.stderr).trim()))
    }
}

/// Writes through a hidden temp file and a rename, so a crash mid-write leaves the old file whole.
/// `draft_files` skips dot files, so the temp file is never published or backed up.
pub fn write_atomic(path: &Path, content: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let tmp = path.with_file_name(format!(".{name}.tmp"));
    let mut f = fs::File::create(&tmp).map_err(|e| e.to_string())?;
    f.write_all(content).and_then(|_| f.sync_all()).map_err(|e| e.to_string())?;
    fs::rename(&tmp, path).map_err(|e| e.to_string())
}

pub fn check_slug(slug: &str) -> Result<(), String> {
    let ok = !slug.is_empty()
        && slug.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !slug.starts_with('-');
    if ok {
        Ok(())
    } else {
        Err(format!("Invalid slug: {slug:?}"))
    }
}

/// The post's files in a folder: top-level, not hidden.
pub fn draft_files(dir: &Path) -> Result<Vec<String>, String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_file())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| !n.starts_with('.'))
        .collect();
    names.sort();
    Ok(names)
}

/// Blob hashes of the post files in `dir`; empty when `dir` doesn't exist.
fn dir_files(repo: &Path, dir: &Path) -> Result<Files, String> {
    if !dir.is_dir() {
        return Ok(Files::new());
    }
    let names = draft_files(dir)?;
    if names.is_empty() {
        return Ok(Files::new());
    }
    let paths: Vec<String> = names.iter().map(|n| dir.join(n).to_string_lossy().to_string()).collect();
    let mut args = vec!["hash-object", "--no-filters", "--"];
    args.extend(paths.iter().map(String::as_str));
    let hashes = git(repo, &args)?;
    Ok(names.into_iter().zip(hashes.lines().map(str::to_string)).collect())
}

/// Makes `to` hold exactly the post files of `from`.
pub fn replace_files(from: &Path, to: &Path) -> Result<(), String> {
    fs::create_dir_all(to).map_err(|e| e.to_string())?;
    let keep = draft_files(from)?;
    for name in draft_files(to)? {
        if !keep.contains(&name) {
            fs::remove_file(to.join(&name)).map_err(|e| e.to_string())?;
        }
    }
    for name in keep {
        fs::copy(from.join(&name), to.join(&name)).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Copies a whole folder, hidden entries and subfolders included, replacing `to`.
pub fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    if to.exists() {
        fs::remove_dir_all(to).map_err(|e| e.to_string())?;
    }
    fs::create_dir_all(to).map_err(|e| e.to_string())?;
    for entry in fs::read_dir(from).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.is_dir() {
            copy_tree(&path, &to.join(entry.file_name()))?;
        } else {
            fs::copy(&path, to.join(entry.file_name())).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

// ---------- draft metadata ----------

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Debug)]
pub struct Meta {
    /// `post/<slug>` once the draft has been pushed; None while it tracks the main branch.
    pub branch: Option<String>,
    /// The commit the base was taken from.
    pub commit: Option<String>,
}

pub fn read_meta(draft: &Path) -> Meta {
    fs::read_to_string(draft.join(META)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn write_meta(draft: &Path, meta: &Meta) -> Result<(), String> {
    write_atomic(&draft.join(META), serde_json::to_string_pretty(meta).map_err(|e| e.to_string())?.as_bytes())
}

pub fn base_known(draft: &Path) -> bool {
    draft.join(BASE).is_dir()
}

/// Marks a brand-new draft as having an empty base: nothing on the remote yet.
pub fn init_new(draft: &Path) -> Result<(), String> {
    fs::create_dir_all(draft.join(BASE)).map_err(|e| e.to_string())?;
    write_meta(draft, &Meta::default())
}

// ---------- the remote side ----------

/// What the app knows about a GitHub PR, keyed by head branch.
#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PrInfo {
    pub number: u64,
    pub url: String,
    /// "OPEN", "MERGED" or "CLOSED".
    pub state: String,
    /// Head commit of the PR when it was last listed.
    pub head_oid: String,
}

/// Where the repository's posts live, and which branch PRs target.
pub struct Ctx<'a> {
    /// The app's own clone.
    pub repo: &'a Path,
    pub main: &'a str,
    pub posts_dir: &'a str,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Layout {
    /// `<posts_dir>/<slug>/index.md` plus images.
    Folder,
    /// `<posts_dir>/<slug>.md`, shown in the draft as `index.md`.
    File,
}

pub fn rev_sha(repo: &Path, rev: &str) -> Option<String> {
    git(repo, &["rev-parse", "--verify", "--quiet", &format!("{rev}^{{commit}}")]).ok()
}

impl Ctx<'_> {
    pub fn main_rev(&self) -> String {
        format!("origin/{}", self.main)
    }

    /// The remote ref a draft tracks: its pushed branch, unless that branch is gone or its PR merged it into main.
    pub fn upstream(&self, slug: &str, meta: &Meta, prs: &HashMap<String, PrInfo>) -> String {
        let branch = format!("post/{slug}");
        let rev = format!("origin/{branch}");
        if meta.branch.as_deref() == Some(branch.as_str()) {
            if let Some(sha) = rev_sha(self.repo, &rev) {
                let merged = prs.get(&branch).is_some_and(|p| p.state == "MERGED" && p.head_oid == sha);
                if !merged {
                    return rev;
                }
            }
        }
        self.main_rev()
    }

    /// The post's files at `rev`, or None when the post isn't there.
    fn remote(&self, rev: &str, slug: &str) -> Result<Option<(Layout, Files)>, String> {
        let folder = format!("{}/{slug}/", self.posts_dir);
        let tree = git(self.repo, &["ls-tree", rev, "--", &folder])?;
        let files: Files = tree
            .lines()
            .filter_map(|line| {
                let (meta, path) = line.split_once('\t')?;
                let mut parts = meta.split_whitespace();
                let kind = parts.nth(1)?;
                let hash = parts.next()?;
                let name = path.strip_prefix(&folder)?;
                (kind == "blob").then(|| (name.to_string(), hash.to_string()))
            })
            .collect();
        if !files.is_empty() {
            return Ok(Some((Layout::Folder, files)));
        }
        let file = format!("{}/{slug}.md", self.posts_dir);
        let tree = git(self.repo, &["ls-tree", rev, "--", &file])?;
        Ok(tree
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(2))
            .map(|hash| (Layout::File, Files::from([("index.md".to_string(), hash.to_string())]))))
    }

    /// The post's `index.md` text at `rev`, or None when the post isn't there.
    pub fn text_at(&self, rev: &str, slug: &str) -> Result<Option<String>, String> {
        let Some((_, files)) = self.remote(rev, slug)? else { return Ok(None) };
        match files.get("index.md") {
            Some(hash) => Ok(Some(String::from_utf8_lossy(&blob(self.repo, hash)?).to_string())),
            None => Ok(None),
        }
    }

    /// Writes the post's files at `rev` into `dir`, replacing its post files.
    fn write_remote(&self, files: &Files, dir: &Path) -> Result<(), String> {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        for name in draft_files(dir)? {
            if !files.contains_key(&name) {
                fs::remove_file(dir.join(&name)).map_err(|e| e.to_string())?;
            }
        }
        for (name, hash) in files {
            write_atomic(&dir.join(name), &blob(self.repo, hash)?)?;
        }
        Ok(())
    }

    // ---------- status ----------

    pub fn status(&self, slug: &str, draft: &Path, prs: &HashMap<String, PrInfo>) -> Result<Status, String> {
        let meta = read_meta(draft);
        let upstream = self.upstream(slug, &meta, prs);
        let remote = self.remote(&upstream, slug)?;
        let local = dir_files(self.repo, draft)?;
        let known = base_known(draft);
        let base = dir_files(self.repo, &draft.join(BASE))?;
        let remote_files = remote.as_ref().map(|r| r.1.clone()).unwrap_or_default();
        let behind = if !known || remote_files == base {
            0
        } else {
            meta.commit
                .as_deref()
                .filter(|c| rev_sha(self.repo, c).is_some())
                .and_then(|c| {
                    let range = format!("{c}..{upstream}");
                    let (folder, file) = (format!("{}/{slug}", self.posts_dir), format!("{}/{slug}.md", self.posts_dir));
                    git(self.repo, &["rev-list", "--count", &range, "--", &folder, &file]).ok()?.parse().ok()
                })
                .unwrap_or(0)
                .max(1)
        };
        Ok(Status {
            upstream: upstream.trim_start_matches("origin/").to_string(),
            base_known: known,
            changes: if known { self.changes(draft, &base, &local)? } else { 0 },
            behind,
            on_remote: remote.is_some(),
            in_sync: known && remote_files == base && local == base,
        })
    }

    /// Lines added plus removed in index.md, plus one per other file that changed.
    fn changes(&self, draft: &Path, base: &Files, local: &Files) -> Result<u32, String> {
        let mut n = 0;
        for name in base.keys().chain(local.keys()).collect::<BTreeSet<_>>() {
            if base.get(name) == local.get(name) {
                continue;
            }
            if name == "index.md" {
                let path_or_null = |p: PathBuf| if p.exists() { p.to_string_lossy().to_string() } else { "/dev/null".into() };
                let (a, b) = (path_or_null(draft.join(BASE).join(name)), path_or_null(draft.join(name)));
                let out = run(self.repo, "git", &["diff", "--no-index", "--numstat", "--", &a, &b])?;
                let stat = String::from_utf8_lossy(&out.stdout);
                let counted: u32 = stat
                    .split_whitespace()
                    .take(2)
                    .filter_map(|x| x.parse::<u32>().ok())
                    .sum();
                n += counted.max(1);
            } else {
                n += 1;
            }
        }
        Ok(n)
    }

    // ---------- edit ----------

    /// Starts a draft from a post on the main branch.
    pub fn start_from_main(&self, slug: &str, draft: &Path) -> Result<(), String> {
        let rev = self.main_rev();
        let (_, files) = self.remote(&rev, slug)?.ok_or_else(|| format!("{slug} isn't on {}", self.main))?;
        self.write_remote(&files, draft)?;
        self.write_remote(&files, &draft.join(BASE))?;
        write_meta(draft, &Meta { branch: None, commit: rev_sha(self.repo, &rev) })
    }

    // ---------- pull ----------

    /// Brings remote changes into the draft. Nothing is written when there is a conflict; the conflict is
    /// saved in `work` until it is resolved or aborted. Before writing, the whole draft is copied to `undo`.
    pub fn pull(
        &self,
        slug: &str,
        draft: &Path,
        prs: &HashMap<String, PrInfo>,
        work: &Path,
        undo: &Path,
    ) -> Result<PullOutcome, String> {
        let meta = read_meta(draft);
        let upstream = self.upstream(slug, &meta, prs);
        let sha = rev_sha(self.repo, &upstream).ok_or_else(|| format!("{upstream} doesn't exist; sync first"))?;
        let remote = self.remote(&upstream, slug)?.map(|r| r.1).unwrap_or_default();
        let base = base_known(draft).then(|| dir_files(self.repo, &draft.join(BASE))).transpose()?;
        let local = dir_files(self.repo, draft)?;
        let next_meta = Meta {
            branch: (upstream != self.main_rev()).then(|| format!("post/{slug}")),
            commit: Some(sha.clone()),
        };
        if base.as_ref() == Some(&remote) {
            return Ok(PullOutcome::UpToDate);
        }

        if work.exists() {
            fs::remove_dir_all(work).map_err(|e| e.to_string())?;
        }
        let (result_dir, remote_dir) = (work.join("result"), work.join("remote"));
        self.write_remote(&remote, &remote_dir)?;
        replace_files(draft, &result_dir)?;

        let mut text = None;
        let mut binary = Vec::new();
        let mut taken = Vec::new();
        let names: BTreeSet<&String> = local.keys().chain(remote.keys()).chain(base.iter().flat_map(|b| b.keys())).collect();
        for name in names {
            let (l, r) = (local.get(name), remote.get(name));
            let b = base.as_ref().map(|b| b.get(name));
            if l == r || b == Some(r) {
                continue; // Same on both sides, or only changed locally.
            }
            if b == Some(l) {
                // Only changed remotely: take the remote side.
                match r {
                    Some(_) => fs::copy(remote_dir.join(name), result_dir.join(name)).map(drop),
                    None => fs::remove_file(result_dir.join(name)),
                }
                .map_err(|e| e.to_string())?;
                taken.push(name.clone());
                continue;
            }
            // Changed on both sides.
            if name.ends_with(".md") && l.is_some() && r.is_some() {
                let empty = work.join("empty");
                fs::write(&empty, "").map_err(|e| e.to_string())?;
                let base_path = match b {
                    Some(Some(_)) => draft.join(BASE).join(name),
                    _ => empty,
                };
                let merge = |favor: &str| -> Result<(String, bool), String> {
                    let (ours, theirs) = (draft.join(name), remote_dir.join(name));
                    let (o, bp, t) = (ours.to_string_lossy(), base_path.to_string_lossy(), theirs.to_string_lossy());
                    let mut args = vec!["merge-file", "-p", "-L", "yours", "-L", "base", "-L", "remote"];
                    if !favor.is_empty() {
                        args.push(favor);
                    }
                    args.extend([o.as_ref(), bp.as_ref(), t.as_ref()]);
                    let out = run(self.repo, "git", &args)?;
                    match out.status.code() {
                        Some(code) if code >= 0 => Ok((String::from_utf8_lossy(&out.stdout).to_string(), code > 0)),
                        _ => Err(format!("git merge-file failed: {}", String::from_utf8_lossy(&out.stderr).trim())),
                    }
                };
                let (merged, conflicted) = merge("")?;
                if conflicted {
                    let (ours, _) = merge("--ours")?;
                    let (theirs, _) = merge("--theirs")?;
                    text = Some(TextConflict { name: name.clone(), ours, theirs });
                } else {
                    write_atomic(&result_dir.join(name), merged.as_bytes())?;
                    taken.push(name.clone());
                }
            } else {
                binary.push(name.clone());
            }
        }

        let conflict = Conflict { upstream: upstream.clone(), text, binary };
        if conflict.text.is_some() || !conflict.binary.is_empty() {
            fs::write(work.join("state.json"), serde_json::to_string(&(&conflict, &next_meta)).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
            return Ok(PullOutcome::Conflict(conflict));
        }
        copy_tree(draft, undo)?;
        apply(draft, &result_dir, &remote_dir, &next_meta)?;
        fs::remove_dir_all(work).map_err(|e| e.to_string())?;
        Ok(PullOutcome::Updated { files: taken })
    }

    // ---------- push ----------

    /// Commits the draft to `post/<slug>` and pushes it. Refuses when the remote no longer matches the base,
    /// and the push carries a lease, so GitHub also refuses it if the branch moved since the last fetch.
    pub fn push(&self, slug: &str, draft: &Path, prs: &HashMap<String, PrInfo>) -> Result<String, String> {
        if !base_known(draft) {
            return Err("The remote version of this post is unknown here. Pull first.".into());
        }
        let meta = read_meta(draft);
        let upstream = self.upstream(slug, &meta, prs);
        let start = rev_sha(self.repo, &upstream).ok_or_else(|| format!("{upstream} doesn't exist; sync first"))?;
        let remote = self.remote(&upstream, slug)?;
        let base = dir_files(self.repo, &draft.join(BASE))?;
        if remote.as_ref().map(|r| &r.1).unwrap_or(&Files::new()) != &base {
            return Err(format!("{} has changes you don't have yet. Pull first.", upstream.trim_start_matches("origin/")));
        }
        let local = dir_files(self.repo, draft)?;
        if local == base {
            return Err("Nothing to push: the draft matches the remote".into());
        }

        let branch = format!("post/{slug}");
        // The lease: the branch must still be where the last fetch saw it, or not exist.
        let expected = rev_sha(self.repo, &format!("origin/{branch}")).unwrap_or_default();
        let repo = self.repo;
        git(repo, &["checkout", "--quiet", "--force", "--no-track", "-B", &branch, &start])?;
        git(repo, &["clean", "-fdq"])?;

        let folder = format!("{}/{slug}", self.posts_dir);
        let file = format!("{}/{slug}.md", self.posts_dir);
        let keep_file = matches!(remote, Some((Layout::File, _))) && local.len() == 1 && local.contains_key("index.md");
        if let Some((layout, files)) = &remote {
            match layout {
                Layout::File if !keep_file => drop(git(repo, &["rm", "--quiet", "--", &file])?),
                Layout::Folder => {
                    for name in files.keys().filter(|n| !local.contains_key(*n)) {
                        git(repo, &["rm", "--quiet", "--", &format!("{folder}/{name}")])?;
                    }
                }
                _ => {}
            }
        }
        if keep_file {
            fs::copy(draft.join("index.md"), repo.join(&file)).map_err(|e| e.to_string())?;
            git(repo, &["add", "--", &file])?;
        } else {
            fs::create_dir_all(repo.join(&folder)).map_err(|e| e.to_string())?;
            for name in local.keys() {
                fs::copy(draft.join(name), repo.join(&folder).join(name)).map_err(|e| e.to_string())?;
            }
            git(repo, &["add", "--all", "--", &folder])?;
        }
        git(repo, &["commit", "--quiet", "-m", &format!("post: {slug}")])?;
        git(
            repo,
            &[
                "push",
                "--quiet",
                &format!("--force-with-lease=refs/heads/{branch}:{expected}"),
                "origin",
                &format!("HEAD:refs/heads/{branch}"),
            ],
        )
        .map_err(|e| {
            if e.contains("stale info") || e.contains("rejected") {
                format!("{branch} changed on GitHub since the last sync. Pull first.")
            } else {
                e
            }
        })?;
        let sha = git(repo, &["rev-parse", "HEAD"])?;
        replace_files(draft, &draft.join(BASE))?;
        write_meta(draft, &Meta { branch: Some(branch), commit: Some(sha.clone()) })?;
        Ok(sha)
    }
}

/// Writes a pull's result into the draft and makes the remote files its new base.
fn apply(draft: &Path, result: &Path, remote: &Path, meta: &Meta) -> Result<(), String> {
    for name in draft_files(draft)? {
        if !result.join(&name).exists() {
            fs::remove_file(draft.join(&name)).map_err(|e| e.to_string())?;
        }
    }
    for name in draft_files(result)? {
        write_atomic(&draft.join(&name), &fs::read(result.join(&name)).map_err(|e| e.to_string())?)?;
    }
    replace_files(remote, &draft.join(BASE))?;
    write_meta(draft, meta)
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// Branch the draft tracks: the main branch or `post/<slug>`.
    pub upstream: String,
    pub base_known: bool,
    /// Lines changed in index.md plus other files changed, against the base.
    pub changes: u32,
    /// Commits on the upstream that changed the post since the base; at least 1 when it differs.
    pub behind: u32,
    /// The post exists on the upstream.
    pub on_remote: bool,
    /// Draft, base and remote are all the same.
    pub in_sync: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TextConflict {
    pub name: String,
    /// The merge with every conflicting chunk resolved to your side; clean remote changes are already in.
    pub ours: String,
    /// The same merge with conflicting chunks resolved to the remote side.
    pub theirs: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Conflict {
    pub upstream: String,
    pub text: Option<TextConflict>,
    /// Images changed on both sides.
    pub binary: Vec<String>,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum PullOutcome {
    UpToDate,
    Updated { files: Vec<String> },
    Conflict(Conflict),
}

pub fn read_conflict(work: &Path) -> Option<Conflict> {
    let s = fs::read_to_string(work.join("state.json")).ok()?;
    serde_json::from_str::<(Conflict, Meta)>(&s).ok().map(|c| c.0)
}

/// Finishes a conflicted pull: `text` replaces the conflicted markdown file, and `remote_images` lists the
/// conflicted images to take from the remote (the rest keep yours).
pub fn resolve(draft: &Path, work: &Path, undo: &Path, text: Option<&str>, remote_images: &[String]) -> Result<(), String> {
    let s = fs::read_to_string(work.join("state.json")).map_err(|_| "No conflict to resolve".to_string())?;
    let (conflict, meta): (Conflict, Meta) = serde_json::from_str(&s).map_err(|e| e.to_string())?;
    let (result, remote) = (work.join("result"), work.join("remote"));
    if let Some(tc) = &conflict.text {
        let text = text.ok_or("The resolved text is missing")?;
        write_atomic(&result.join(&tc.name), text.as_bytes())?;
    }
    for name in conflict.binary.iter().filter(|n| remote_images.contains(n)) {
        match remote.join(name).exists() {
            true => fs::copy(remote.join(name), result.join(name)).map(drop),
            false => fs::remove_file(result.join(name)),
        }
        .map_err(|e| e.to_string())?;
    }
    copy_tree(draft, undo)?;
    apply(draft, &result, &remote, &meta)?;
    fs::remove_dir_all(work).map_err(|e| e.to_string())
}

/// Puts the draft back exactly as it was before the last pull.
pub fn undo_pull(draft: &Path, undo: &Path) -> Result<(), String> {
    if !undo.is_dir() {
        return Err("Nothing to undo".into());
    }
    copy_tree(undo, draft)?;
    fs::remove_dir_all(undo).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bare repo standing in for GitHub, the app's clone, and a second clone for someone editing on GitHub.
    struct World {
        root: PathBuf,
    }

    impl World {
        fn new(name: &str) -> World {
            std::env::set_var("GIT_CONFIG_GLOBAL", "/dev/null");
            std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
            for (k, v) in [("GIT_AUTHOR_NAME", "t"), ("GIT_AUTHOR_EMAIL", "t@t"), ("GIT_COMMITTER_NAME", "t"), ("GIT_COMMITTER_EMAIL", "t@t")] {
                std::env::set_var(k, v);
            }
            let root = std::env::temp_dir().join(format!("mdwriter-test-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            git(&root, &["init", "--quiet", "--bare", "-b", "main", "origin.git"]).unwrap();
            git(&root, &["clone", "--quiet", "origin.git", "other"]).unwrap();
            let w = World { root };
            w.write_other("src/content/posts/hello/index.md", &numbered("hello"));
            w.write_other("src/content/posts/hello/a.png", "png-1");
            w.write_other("src/content/posts/single.md", &numbered("single"));
            w.commit_other("main", "seed");
            git(&w.root, &["clone", "--quiet", "origin.git", "app"]).unwrap();
            fs::create_dir_all(w.root.join("drafts")).unwrap();
            w
        }
        fn ctx(&self) -> Ctx<'_> {
            Ctx { repo: Box::leak(self.root.join("app").into_boxed_path()), main: "main", posts_dir: "src/content/posts" }
        }
        fn draft(&self, slug: &str) -> PathBuf {
            self.root.join("drafts").join(slug)
        }
        fn work(&self) -> PathBuf {
            self.root.join("work")
        }
        fn undo(&self) -> PathBuf {
            self.root.join("undo")
        }
        fn other(&self) -> PathBuf {
            self.root.join("other")
        }
        fn write_other(&self, path: &str, content: &str) {
            let p = self.other().join(path);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, content).unwrap();
        }
        /// Commits everything in the other clone to `branch` (based on origin's copy of it, or main) and pushes.
        fn commit_other(&self, branch: &str, msg: &str) {
            let o = self.other();
            git(&o, &["add", "--all"]).unwrap();
            git(&o, &["commit", "--quiet", "-m", msg]).unwrap();
            git(&o, &["push", "--quiet", "origin", &format!("HEAD:refs/heads/{branch}")]).unwrap();
        }
        fn other_checkout(&self, branch: &str) {
            let o = self.other();
            git(&o, &["fetch", "--quiet", "origin"]).unwrap();
            git(&o, &["checkout", "--quiet", "--force", "-B", branch, &format!("origin/{branch}")]).unwrap();
        }
        fn fetch(&self) {
            git(&self.root.join("app"), &["fetch", "--quiet", "--prune", "origin"]).unwrap();
        }
        fn read(&self, slug: &str, name: &str) -> String {
            fs::read_to_string(self.draft(slug).join(name)).unwrap()
        }
        fn edit_line(&self, slug: &str, line: usize, text: &str) {
            let p = self.draft(slug).join("index.md");
            fs::write(&p, replace_line(&fs::read_to_string(&p).unwrap(), line, text)).unwrap();
        }
        fn remote_text(&self, rev: &str, path: &str) -> String {
            git(&self.root.join("origin.git"), &["show", &format!("{rev}:{path}")]).unwrap()
        }
    }

    impl Drop for World {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    /// Twenty numbered lines, so edits far apart merge cleanly and edits to one line conflict.
    fn numbered(title: &str) -> String {
        let mut s = format!("---\ntitle: \"{title}\"\n---\n");
        for i in 1..=20 {
            s.push_str(&format!("line {i}\n"));
        }
        s
    }

    fn replace_line(text: &str, n: usize, with: &str) -> String {
        text.lines()
            .map(|l| if l == format!("line {n}") { with.to_string() } else { l.to_string() })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    }

    fn no_prs() -> HashMap<String, PrInfo> {
        HashMap::new()
    }

    fn snapshot(dir: &Path) -> BTreeMap<String, Vec<u8>> {
        fn walk(dir: &Path, prefix: &str, out: &mut BTreeMap<String, Vec<u8>>) {
            for e in fs::read_dir(dir).unwrap() {
                let e = e.unwrap();
                let name = format!("{prefix}{}", e.file_name().to_string_lossy());
                if e.path().is_dir() {
                    walk(&e.path(), &format!("{name}/"), out);
                } else {
                    out.insert(name, fs::read(e.path()).unwrap());
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(dir, "", &mut out);
        out
    }

    #[test]
    fn push_is_refused_when_the_remote_changed_and_pull_then_merges() {
        let w = World::new("stale");
        let ctx = w.ctx();
        ctx.start_from_main("hello", &w.draft("hello")).unwrap();
        w.edit_line("hello", 2, "line 2 mine");

        // Someone changes the post on main.
        w.other_checkout("main");
        w.write_other("src/content/posts/hello/index.md", &replace_line(&numbered("hello"), 19, "line 19 theirs"));
        w.commit_other("main", "edit on main");
        w.fetch();

        let st = ctx.status("hello", &w.draft("hello"), &no_prs()).unwrap();
        assert_eq!((st.behind, st.changes > 0), (1, true));
        let err = ctx.push("hello", &w.draft("hello"), &no_prs()).unwrap_err();
        assert!(err.contains("Pull first"), "{err}");
        assert!(rev_sha(&w.root.join("app"), "origin/post/hello").is_none(), "nothing was pushed");

        // A clean merge keeps both edits, then the push goes through.
        let out = ctx.pull("hello", &w.draft("hello"), &no_prs(), &w.work(), &w.undo()).unwrap();
        assert!(matches!(out, PullOutcome::Updated { .. }), "{out:?}");
        let text = w.read("hello", "index.md");
        assert!(text.contains("line 2 mine") && text.contains("line 19 theirs"), "{text}");
        ctx.push("hello", &w.draft("hello"), &no_prs()).unwrap();
        let pushed = w.remote_text("post/hello", "src/content/posts/hello/index.md");
        assert!(pushed.contains("line 2 mine") && pushed.contains("line 19 theirs"));
        assert_eq!(w.remote_text("post/hello", "src/content/posts/hello/a.png"), "png-1");
    }

    #[test]
    fn the_lease_refuses_a_push_over_a_branch_that_moved_after_the_last_fetch() {
        let w = World::new("lease");
        let ctx = w.ctx();
        ctx.start_from_main("hello", &w.draft("hello")).unwrap();
        w.edit_line("hello", 2, "line 2 v1");
        ctx.push("hello", &w.draft("hello"), &no_prs()).unwrap();

        // Someone accepts a suggestion on the PR branch; the app hasn't fetched since.
        w.other_checkout("post/hello");
        w.write_other("src/content/posts/hello/index.md", &replace_line(&w.remote_text("post/hello", "src/content/posts/hello/index.md"), 5, "line 5 reviewer"));
        w.commit_other("post/hello", "suggestion");
        let theirs = w.remote_text("post/hello", "src/content/posts/hello/index.md");

        w.edit_line("hello", 3, "line 3 v2");
        let err = ctx.push("hello", &w.draft("hello"), &no_prs()).unwrap_err();
        assert!(err.contains("Pull first"), "{err}");
        assert_eq!(w.remote_text("post/hello", "src/content/posts/hello/index.md"), theirs, "the reviewer's commit survived");
    }

    #[test]
    fn a_conflict_writes_nothing_until_resolved_and_abort_leaves_the_draft_exact() {
        let w = World::new("conflict");
        let ctx = w.ctx();
        let draft = w.draft("hello");
        ctx.start_from_main("hello", &draft).unwrap();
        w.edit_line("hello", 4, "line 4 mine");
        w.edit_line("hello", 12, "line 12 mine only");
        fs::write(draft.join("a.png"), "png-mine").unwrap();

        w.other_checkout("main");
        w.write_other("src/content/posts/hello/index.md", &replace_line(&replace_line(&numbered("hello"), 4, "line 4 theirs"), 18, "line 18 theirs only"));
        w.write_other("src/content/posts/hello/a.png", "png-theirs");
        w.commit_other("main", "conflicting edit");
        w.fetch();

        let before = snapshot(&draft);
        let PullOutcome::Conflict(c) = ctx.pull("hello", &draft, &no_prs(), &w.work(), &w.undo()).unwrap() else {
            panic!("expected a conflict")
        };
        assert_eq!(snapshot(&draft), before, "a conflicted pull writes nothing to the draft");
        assert_eq!(c.binary, vec!["a.png".to_string()]);
        let tc = c.text.unwrap();
        // Both sides already carry the clean changes; they differ only in the conflicting line.
        for side in [&tc.ours, &tc.theirs] {
            assert!(side.contains("line 12 mine only") && side.contains("line 18 theirs only"), "{side}");
        }
        assert!(tc.ours.contains("line 4 mine") && tc.theirs.contains("line 4 theirs"));
        assert!(read_conflict(&w.work()).is_some());

        // Abort: drop the conflict; the draft is untouched.
        fs::remove_dir_all(w.work()).unwrap();
        assert_eq!(snapshot(&draft), before);

        // Pull again and resolve: take the remote image and a hand-merged line.
        let PullOutcome::Conflict(c) = ctx.pull("hello", &draft, &no_prs(), &w.work(), &w.undo()).unwrap() else { panic!() };
        let resolved = c.text.unwrap().ours.replace("line 4 mine", "line 4 both");
        resolve(&draft, &w.work(), &w.undo(), Some(&resolved), &["a.png".to_string()]).unwrap();
        assert_eq!(w.read("hello", "index.md"), resolved);
        assert_eq!(w.read("hello", "a.png"), "png-theirs");
        let st = ctx.status("hello", &draft, &no_prs()).unwrap();
        assert_eq!(st.behind, 0, "the base is now the remote");
        ctx.push("hello", &draft, &no_prs()).unwrap();

        // Undo puts back the exact draft from before the resolve, base included.
        undo_pull(&draft, &w.undo()).unwrap();
        assert_eq!(snapshot(&draft), before);
    }

    #[test]
    fn undo_restores_the_exact_draft_after_a_clean_pull() {
        let w = World::new("undo");
        let ctx = w.ctx();
        let draft = w.draft("hello");
        ctx.start_from_main("hello", &draft).unwrap();
        w.edit_line("hello", 1, "line 1 mine");
        w.other_checkout("main");
        w.write_other("src/content/posts/hello/index.md", &replace_line(&numbered("hello"), 20, "line 20 theirs"));
        w.write_other("src/content/posts/hello/b.png", "new image");
        w.commit_other("main", "remote edit");
        w.fetch();

        let before = snapshot(&draft);
        let out = ctx.pull("hello", &draft, &no_prs(), &w.work(), &w.undo()).unwrap();
        assert!(matches!(out, PullOutcome::Updated { .. }));
        assert_eq!(w.read("hello", "b.png"), "new image");
        assert_ne!(snapshot(&draft), before);
        undo_pull(&draft, &w.undo()).unwrap();
        assert_eq!(snapshot(&draft), before);
    }

    #[test]
    fn a_draft_with_an_unknown_base_never_pushes_and_every_difference_is_a_conflict() {
        let w = World::new("unknown");
        let ctx = w.ctx();
        let draft = w.draft("hello");
        ctx.start_from_main("hello", &draft).unwrap();
        // As if restored from an old backup: no base, no metadata.
        fs::remove_dir_all(draft.join(BASE)).unwrap();
        fs::remove_file(draft.join(META)).unwrap();
        w.edit_line("hello", 6, "line 6 mine");

        let st = ctx.status("hello", &draft, &no_prs()).unwrap();
        assert!(!st.base_known && st.on_remote);
        assert!(ctx.push("hello", &draft, &no_prs()).unwrap_err().contains("Pull first"));

        let PullOutcome::Conflict(c) = ctx.pull("hello", &draft, &no_prs(), &w.work(), &w.undo()).unwrap() else {
            panic!("an unknown base must not merge silently")
        };
        let tc = c.text.unwrap();
        assert!(tc.ours.contains("line 6 mine") && !tc.theirs.contains("line 6 mine"));
        assert!(c.binary.is_empty(), "a.png is identical on both sides");

        // When the draft already matches the remote, pulling just records the base.
        fs::remove_dir_all(w.work()).unwrap();
        fs::write(draft.join("index.md"), w.read("hello", "index.md").replace("line 6 mine", "line 6")).unwrap();
        assert!(matches!(ctx.pull("hello", &draft, &no_prs(), &w.work(), &w.undo()).unwrap(), PullOutcome::Updated { .. }));
        assert!(ctx.status("hello", &draft, &no_prs()).unwrap().in_sync);
    }

    #[test]
    fn single_file_posts_stay_single_files_until_they_get_images() {
        let w = World::new("single");
        let ctx = w.ctx();
        let draft = w.draft("single");
        ctx.start_from_main("single", &draft).unwrap();
        w.edit_line("single", 3, "line 3 edited");
        ctx.push("single", &draft, &no_prs()).unwrap();
        assert!(w.remote_text("post/single", "src/content/posts/single.md").contains("line 3 edited"));

        fs::write(draft.join("pic.png"), "pic").unwrap();
        w.fetch();
        ctx.push("single", &draft, &no_prs()).unwrap();
        assert!(w.remote_text("post/single", "src/content/posts/single/index.md").contains("line 3 edited"));
        assert!(git(&w.root.join("origin.git"), &["show", "post/single:src/content/posts/single.md"]).is_err());
    }

    #[test]
    fn a_new_post_pushes_to_its_own_branch_and_tracks_it() {
        let w = World::new("new");
        let ctx = w.ctx();
        let draft = w.draft("fresh");
        fs::create_dir_all(&draft).unwrap();
        fs::write(draft.join("index.md"), "hi\n").unwrap();
        init_new(&draft).unwrap();
        let st = ctx.status("fresh", &draft, &no_prs()).unwrap();
        assert!(st.base_known && !st.on_remote && st.changes > 0);
        ctx.push("fresh", &draft, &no_prs()).unwrap();
        let st = ctx.status("fresh", &draft, &no_prs()).unwrap();
        assert_eq!((st.upstream.as_str(), st.in_sync), ("post/fresh", true));
        // Once its PR is merged at that commit, the draft tracks main again.
        let sha = rev_sha(&w.root.join("app"), "origin/post/fresh").unwrap();
        let prs = HashMap::from([("post/fresh".to_string(), PrInfo { number: 1, url: String::new(), state: "MERGED".into(), head_oid: sha })]);
        assert_eq!(ctx.upstream("fresh", &read_meta(&draft), &prs), "origin/main");
    }
}

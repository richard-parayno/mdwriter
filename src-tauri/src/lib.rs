//! Backend for mdwriter.
//!
//! Drafts live in the app data dir (`drafts/<slug>/index.md` plus images). All git work happens in an
//! app-owned clone (`repo/`), never in the user's own checkout. Every action that changes a draft or the
//! remote is explicit (Pull, Push, Open PR); listing posts only reads. The git logic is in `sync.rs`.

mod sync;

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use sync::{check_slug, draft_files, exec, git, write_atomic, PrInfo};
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent};

const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "avif", "svg"];

/// User settings, stored as `settings.json` in the app data dir.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Settings {
    /// GitHub repository as `owner/name`; cloned over SSH.
    repo: String,
    /// Branch that posts are read from and PRs target.
    branch: String,
    /// Folder in the repo that holds the posts.
    posts_dir: String,
    /// Live URL of a post, with `{slug}` in place of the slug. Empty hides the Site button.
    post_url: String,
    editor_font_size: u32,
    /// Hide markdown syntax away from the cursor and show images inline.
    live_preview: bool,
    line_numbers: bool,
    /// Copy drafts and settings to `iCloud Drive/mdwriter` as they save.
    backup: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            repo: "richard-parayno/blog".into(),
            branch: "main".into(),
            posts_dir: "src/content/posts".into(),
            post_url: "https://richardparayno.com/blog/{slug}/".into(),
            editor_font_size: 14,
            live_preview: true,
            line_numbers: true,
            backup: false,
        }
    }
}

impl Settings {
    fn remote(&self) -> String {
        format!("git@github.com:{}.git", self.repo)
    }
    fn base(&self) -> String {
        format!("origin/{}", self.branch)
    }
}

#[derive(Default)]
struct AppState {
    /// Serializes every operation on the app-owned clone.
    repo_lock: Mutex<()>,
    /// PRs from the last `gh pr list` and when that sync finished; kept in `sync.json` for instant loads.
    cache: Mutex<SyncCache>,
    /// Unix time in ms of the last successful backup, and the last backup error.
    backup: Mutex<(Option<u64>, Option<String>)>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SyncCache {
    /// The latest PR for each head branch.
    prs: HashMap<String, PrInfo>,
    synced_at: Option<u64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PostSummary {
    slug: String,
    /// The post is a local draft the app can edit.
    local: bool,
    content: String,
    /// Sync state of a local draft; None for posts that are only on the main branch.
    sync: Option<sync::Status>,
    /// The open PR for `post/<slug>`.
    pr: Option<PrInfo>,
    /// The post exists on the main branch.
    on_main: bool,
    /// A pull is waiting for its conflicts to be resolved.
    conflict: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PostList {
    posts: Vec<PostSummary>,
    warnings: Vec<String>,
    synced_at: Option<u64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PostFiles {
    dir: String,
    images: Vec<String>,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}


// ---------- paths and process helpers ----------

fn data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

fn drafts_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = data_dir(app)?.join("drafts");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

fn repo_dir(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(data_dir(app)?.join("repo"))
}


fn draft_path(app: &AppHandle, slug: &str) -> Result<PathBuf, String> {
    check_slug(slug)?;
    Ok(drafts_dir(app)?.join(slug))
}



fn settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(data_dir(app)?.join("settings.json"))
}

/// Saved settings, or the defaults when none are saved yet.
fn load_settings(app: &AppHandle) -> Result<Settings, String> {
    match fs::read_to_string(settings_path(app)?) {
        Ok(json) => serde_json::from_str(&json).map_err(|e| format!("settings.json is invalid: {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
        Err(e) => Err(e.to_string()),
    }
}

/// Trims the settings and rejects values that would break git, gh, or the post paths.
fn validate(app: &AppHandle, mut s: Settings) -> Result<Settings, String> {
    s.repo = s.repo.trim().trim_end_matches(".git").to_string();
    s.branch = s.branch.trim().to_string();
    s.posts_dir = s.posts_dir.trim().trim_matches('/').to_string();
    s.post_url = s.post_url.trim().to_string();

    let part_ok = |p: &str| {
        !p.is_empty() && !p.starts_with('.') && p.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
    };
    match s.repo.split_once('/') {
        Some((owner, name)) if part_ok(owner) && part_ok(name) => {}
        _ => return Err("Repository must look like owner/name".into()),
    }
    if s.branch.is_empty()
        || s.branch.starts_with('-')
        || exec(&data_dir(app)?, "git", &["check-ref-format", "--branch", &s.branch]).is_err()
    {
        return Err(format!("{:?} is not a valid branch name", s.branch));
    }
    if s.posts_dir.is_empty() || !Path::new(&s.posts_dir).components().all(|c| matches!(c, Component::Normal(_))) {
        return Err("Posts folder must be a path inside the repository".into());
    }
    if !s.post_url.is_empty()
        && !((s.post_url.starts_with("https://") || s.post_url.starts_with("http://")) && s.post_url.contains("{slug}"))
    {
        return Err("Post URL must start with https:// and contain {slug}".into());
    }
    if !(10..=24).contains(&s.editor_font_size) {
        return Err("Editor font size must be between 10 and 24".into());
    }
    let heads = exec(&data_dir(app)?, "git", &["ls-remote", "--heads", &s.remote(), &s.branch])
        .map_err(|e| format!("Could not reach {}: {e}", s.repo))?;
    if !heads.lines().any(|l| l.ends_with(&format!("refs/heads/{}", s.branch))) {
        return Err(format!("{} has no branch named {}", s.repo, s.branch));
    }
    Ok(s)
}

/// Clones the blog into the app data dir on first use.
fn ensure_repo(app: &AppHandle, s: &Settings) -> Result<PathBuf, String> {
    let repo = repo_dir(app)?;
    if !repo.join(".git").exists() {
        exec(&data_dir(app)?, "git", &["clone", "--quiet", &s.remote(), "repo"])?;
    }
    Ok(repo)
}

/// Lists the repository's PRs with gh and records them and the sync time.
fn refresh_prs(app: &AppHandle, cwd: &Path, s: &Settings) -> Result<(), String> {
    let json = exec(
        cwd,
        "gh",
        &[
            "pr", "list", "--repo", &s.repo, "--state", "all", "--limit", "200", "--json",
            "number,url,headRefName,headRefOid,state",
        ],
    )?;
    let rows: Vec<serde_json::Value> = serde_json::from_str(&json).map_err(|e| e.to_string())?;
    let mut prs: HashMap<String, PrInfo> = HashMap::new();
    for row in rows {
        let (Some(head), Some(number), Some(url), Some(state), Some(oid)) = (
            row["headRefName"].as_str(),
            row["number"].as_u64(),
            row["url"].as_str(),
            row["state"].as_str(),
            row["headRefOid"].as_str(),
        ) else {
            continue;
        };
        // Keep the newest PR per branch.
        if prs.get(head).is_none_or(|p| p.number < number) {
            let pr = PrInfo { number, url: url.into(), state: state.into(), head_oid: oid.into() };
            prs.insert(head.to_string(), pr);
        }
    }
    let state: State<AppState> = app.state();
    let mut cache = state.cache.lock().unwrap();
    *cache = SyncCache { prs, synced_at: Some(now_ms()) };
    let json = serde_json::to_string(&*cache).map_err(|e| e.to_string())?;
    write_atomic(&data_dir(app)?.join("sync.json"), json.as_bytes())
}

/// Fetches and lists PRs; what every Pull and Push starts with.
fn sync_remote(app: &AppHandle, repo: &Path, s: &Settings) -> Result<(), String> {
    git(repo, &["fetch", "--quiet", "--prune", "origin"])?;
    refresh_prs(app, repo, s)
}

fn prs(app: &AppHandle) -> HashMap<String, PrInfo> {
    app.state::<AppState>().cache.lock().unwrap().prs.clone()
}

fn ctx<'a>(repo: &'a Path, s: &'a Settings) -> sync::Ctx<'a> {
    sync::Ctx { repo, main: &s.branch, posts_dir: &s.posts_dir }
}

/// Where a conflicted pull waits to be resolved.
fn conflict_dir(app: &AppHandle, slug: &str) -> Result<PathBuf, String> {
    check_slug(slug)?;
    Ok(data_dir(app)?.join("conflicts").join(slug))
}

/// The draft as it was before the last pull, for Undo.
fn undo_dir(app: &AppHandle, slug: &str) -> Result<PathBuf, String> {
    check_slug(slug)?;
    Ok(data_dir(app)?.join("undo").join(slug))
}

/// Runs `f` off the main thread with the clone locked, after loading settings and making sure the clone exists.
async fn with_repo<T: Send + 'static>(
    app: AppHandle,
    f: impl FnOnce(&AppHandle, &Path, &Settings) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state: State<AppState> = app.state();
        let _guard = state.repo_lock.lock().unwrap();
        let s = load_settings(&app)?;
        let repo = ensure_repo(&app, &s)?;
        f(&app, &repo, &s)
    })
    .await
    .map_err(|e| e.to_string())?
}


/// Maps `src/content/posts/foo.md` and `src/content/posts/foo/index.md` to `foo`,
/// the same ID Astro's glob loader gives them.
fn post_id(posts_dir: &str, path: &str) -> Option<String> {
    let rel = path.strip_prefix(posts_dir)?.strip_prefix('/')?;
    let id = rel.strip_suffix(".md")?;
    Some(id.strip_suffix("/index").unwrap_or(id).to_string())
}

/// Post files on the base branch, as id -> path.
fn base_posts(repo: &Path, s: &Settings) -> Result<HashMap<String, String>, String> {
    let out = git(repo, &["ls-tree", "-r", "--name-only", &s.base(), "--", &s.posts_dir])?;
    Ok(out
        .lines()
        .filter_map(|p| post_id(&s.posts_dir, p).map(|id| (id, p.to_string())))
        .collect())
}



// ---------- commands ----------

/// Lists drafts and posts. Without `fetch` this reads only local state, so it returns at once; with it,
/// it fetches and lists PRs first. Either way it never changes a draft or the remote.
#[tauri::command]
async fn list_posts(app: AppHandle, fetch: bool) -> Result<PostList, String> {
    tauri::async_runtime::spawn_blocking(move || list_posts_blocking(&app, fetch))
        .await
        .map_err(|e| e.to_string())?
}

fn list_posts_blocking(app: &AppHandle, fetch: bool) -> Result<PostList, String> {
    let state: State<AppState> = app.state();
    let _guard = state.repo_lock.lock().unwrap();
    let mut warnings = Vec::new();
    let s = load_settings(app)?;

    let repo = match ensure_repo(app, &s) {
        Ok(repo) => Some(repo),
        Err(e) => {
            warnings.push(e);
            None
        }
    };
    if let (true, Some(repo)) = (fetch, &repo) {
        if let Err(e) = sync_remote(app, repo, &s) {
            warnings.push(e);
        }
    }
    let on_main = match &repo {
        Some(repo) => base_posts(repo, &s).unwrap_or_else(|e| {
            warnings.push(e);
            HashMap::new()
        }),
        None => HashMap::new(),
    };
    let (prs, synced_at) = {
        let cache = state.cache.lock().unwrap();
        (cache.prs.clone(), cache.synced_at)
    };
    let open_pr = |slug: &str| prs.get(&format!("post/{slug}")).filter(|p| p.state == "OPEN").cloned();

    let drafts = drafts_dir(app)?;
    let local = local_drafts(&drafts)?;
    let local_set: HashSet<&String> = local.iter().collect();
    let mut posts = Vec::new();
    for slug in &local {
        let dir = drafts.join(slug);
        let status = repo.as_ref().map(|repo| ctx(repo, &s).status(slug, &dir, &prs)).transpose().unwrap_or_else(|e| {
            warnings.push(format!("{slug}: {e}"));
            None
        });
        posts.push(PostSummary {
            slug: slug.clone(),
            local: true,
            content: fs::read_to_string(dir.join("index.md")).unwrap_or_default(),
            sync: status,
            pr: open_pr(slug),
            on_main: on_main.contains_key(slug),
            conflict: conflict_dir(app, slug)?.join("state.json").exists(),
        });
    }
    if let Some(repo) = &repo {
        let mut remote: Vec<(&String, &String)> = on_main.iter().filter(|(id, _)| !local_set.contains(id)).collect();
        remote.sort();
        for (id, path) in remote {
            posts.push(PostSummary {
                slug: id.clone(),
                local: false,
                content: git(repo, &["show", &format!("{}:{path}", s.base())]).unwrap_or_default(),
                sync: None,
                pr: open_pr(id),
                on_main: true,
                conflict: false,
            });
        }
    }
    Ok(PostList { posts, warnings, synced_at })
}


#[tauri::command]
fn create_draft(app: AppHandle, state: State<AppState>, slug: String, content: String) -> Result<(), String> {
    let dir = draft_path(&app, &slug)?;
    if dir.exists() {
        return Err(format!("A draft named {slug} already exists"));
    }
    let repo = repo_dir(&app)?;
    if repo.join(".git").exists() {
        let _guard = state.repo_lock.lock().unwrap();
        let s = load_settings(&app)?;
        if base_posts(&repo, &s)?.contains_key(&slug) {
            return Err(format!("A post with the slug {slug} is already on {}", s.branch));
        }
    }
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    write_atomic(&dir.join("index.md"), content.as_bytes())?;
    sync::init_new(&dir)?;
    backup_draft(&app, &slug);
    Ok(())
}

#[tauri::command]
fn read_draft(app: AppHandle, slug: String) -> Result<String, String> {
    fs::read_to_string(draft_path(&app, &slug)?.join("index.md")).map_err(|e| e.to_string())
}

#[tauri::command]
fn save_draft(app: AppHandle, slug: String, content: String) -> Result<(), String> {
    let path = draft_path(&app, &slug)?.join("index.md");
    if !path.exists() {
        return Err(format!("No draft named {slug}"));
    }
    write_atomic(&path, content.as_bytes())?;
    backup_draft(&app, &slug);
    Ok(())
}

#[tauri::command]
fn post_files(app: AppHandle, slug: String) -> Result<PostFiles, String> {
    let dir = draft_path(&app, &slug)?;
    let images = draft_files(&dir)?
        .into_iter()
        .filter(|n| n != "index.md")
        .collect();
    Ok(PostFiles { dir: dir.to_string_lossy().to_string(), images })
}

/// Copies a dropped image beside the post's index.md and returns its file name.
#[tauri::command]
fn add_image(app: AppHandle, slug: String, source: String) -> Result<String, String> {
    let dir = draft_path(&app, &slug)?;
    let source = PathBuf::from(source);
    let ext = source
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if !IMAGE_EXTS.contains(&ext.as_str()) {
        return Err(format!("{} is not an image", source.display()));
    }
    let stem: String = source
        .file_stem()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect();
    let stem = stem.trim_matches('-');
    let stem = if stem.is_empty() { "image" } else { stem };
    let mut name = format!("{stem}.{ext}");
    let mut n = 2;
    while dir.join(&name).exists() {
        name = format!("{stem}-{n}.{ext}");
        n += 1;
    }
    fs::copy(&source, dir.join(&name)).map_err(|e| e.to_string())?;
    backup_draft(&app, &slug);
    Ok(name)
}

/// Fetches, then brings remote changes into the draft. A conflict leaves the draft untouched.
#[tauri::command]
async fn pull_post(app: AppHandle, slug: String) -> Result<sync::PullOutcome, String> {
    with_repo(app, move |app, repo, s| {
        sync_remote(app, repo, s)?;
        let out = ctx(repo, s).pull(&slug, &draft_path(app, &slug)?, &prs(app), &conflict_dir(app, &slug)?, &undo_dir(app, &slug)?)?;
        backup_draft(app, &slug);
        Ok(out)
    })
    .await
}

/// Fetches, then commits the draft to `post/<slug>` and pushes it. Refused while the remote has changes.
#[tauri::command]
async fn push_post(app: AppHandle, slug: String) -> Result<String, String> {
    with_repo(app, move |app, repo, s| {
        if conflict_dir(app, &slug)?.join("state.json").exists() {
            return Err("Resolve the conflict first".into());
        }
        sync_remote(app, repo, s)?;
        let sha = ctx(repo, s).push(&slug, &draft_path(app, &slug)?, &prs(app))?;
        backup_draft(app, &slug);
        Ok(sha)
    })
    .await
}

/// Opens a PR from the pushed `post/<slug>` branch into the main branch.
#[tauri::command]
async fn open_pr(app: AppHandle, slug: String, title: String, body: String) -> Result<PrInfo, String> {
    with_repo(app, move |app, repo, s| {
        let branch = format!("post/{slug}");
        if sync::read_meta(&draft_path(app, &slug)?).branch.as_deref() != Some(branch.as_str())
            || sync::rev_sha(repo, &format!("origin/{branch}")).is_none()
        {
            return Err("Push the post first".into());
        }
        let out = exec(
            repo,
            "gh",
            &["pr", "create", "--repo", &s.repo, "--base", &s.branch, "--head", &branch, "--title", &title, "--body", &body],
        )?;
        let url = out.lines().rev().find(|l| l.starts_with("https://")).ok_or_else(|| format!("Unexpected gh output: {out}"))?;
        let number = url.rsplit('/').next().and_then(|n| n.parse().ok()).ok_or_else(|| format!("Unexpected gh output: {url}"))?;
        let pr = PrInfo {
            number,
            url: url.to_string(),
            state: "OPEN".into(),
            head_oid: sync::rev_sha(repo, &format!("origin/{branch}")).unwrap_or_default(),
        };
        app.state::<AppState>().cache.lock().unwrap().prs.insert(branch, pr.clone());
        Ok(pr)
    })
    .await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ConflictView {
    #[serde(flatten)]
    conflict: sync::Conflict,
    /// Folder with the remote side's files, for image thumbnails.
    remote_dir: String,
}

#[tauri::command]
fn conflict_state(app: AppHandle, slug: String) -> Result<Option<ConflictView>, String> {
    let dir = conflict_dir(&app, &slug)?;
    Ok(sync::read_conflict(&dir)
        .map(|conflict| ConflictView { conflict, remote_dir: dir.join("remote").to_string_lossy().to_string() }))
}

/// Writes the resolved text and image choices into the draft; the remote becomes its new base.
#[tauri::command]
fn resolve_conflict(app: AppHandle, slug: String, text: Option<String>, remote_images: Vec<String>) -> Result<(), String> {
    let draft = draft_path(&app, &slug)?;
    sync::resolve(&draft, &conflict_dir(&app, &slug)?, &undo_dir(&app, &slug)?, text.as_deref(), &remote_images)?;
    backup_draft(&app, &slug);
    Ok(())
}

/// Drops a conflicted pull. The draft was never changed, so nothing else happens.
#[tauri::command]
fn abort_conflict(app: AppHandle, slug: String) -> Result<(), String> {
    let dir = conflict_dir(&app, &slug)?;
    if dir.exists() {
        fs::remove_dir_all(dir).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Puts the draft back exactly as it was before the last pull or resolve.
#[tauri::command]
fn undo_pull(app: AppHandle, slug: String) -> Result<(), String> {
    sync::undo_pull(&draft_path(&app, &slug)?, &undo_dir(&app, &slug)?)?;
    backup_draft(&app, &slug);
    Ok(())
}

/// Starts a local draft from a post on the main branch.
#[tauri::command]
async fn edit_post(app: AppHandle, slug: String) -> Result<(), String> {
    with_repo(app, move |app, repo, s| {
        let draft = draft_path(app, &slug)?;
        if draft.exists() {
            return Err(format!("A draft named {slug} already exists"));
        }
        let result = ctx(repo, s).start_from_main(&slug, &draft);
        if result.is_err() {
            let _ = fs::remove_dir_all(&draft);
        }
        result?;
        backup_draft(app, &slug);
        Ok(())
    })
    .await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Commit {
    sha: String,
    /// Unix time in seconds.
    date: u64,
    subject: String,
}

/// Recent commits that changed the post, on the main branch and on `post/<slug>`.
#[tauri::command]
async fn post_history(app: AppHandle, slug: String) -> Result<Vec<Commit>, String> {
    check_slug(&slug)?;
    with_repo(app, move |_, repo, s| {
        let mut args = vec!["log".to_string(), "-n".into(), "30".into(), "--format=%H%x1f%ct%x1f%s".into(), s.base()];
        let branch = format!("origin/post/{slug}");
        if sync::rev_sha(repo, &branch).is_some() {
            args.push(branch);
        }
        args.extend(["--".into(), format!("{}/{slug}", s.posts_dir), format!("{}/{slug}.md", s.posts_dir)]);
        let out = git(repo, &args.iter().map(String::as_str).collect::<Vec<_>>())?;
        Ok(out
            .lines()
            .filter_map(|l| {
                let mut p = l.split('\x1f');
                Some(Commit { sha: p.next()?.into(), date: p.next()?.parse().ok()?, subject: p.next()?.into() })
            })
            .collect())
    })
    .await
}

/// The post's index.md as of `rev`: "base" (last synced), "main", or a commit hash. None when the post
/// didn't exist there.
#[tauri::command]
async fn text_at(app: AppHandle, slug: String, rev: String) -> Result<Option<String>, String> {
    let draft = draft_path(&app, &slug)?;
    if rev == "base" {
        return Ok(sync::base_known(&draft).then(|| fs::read_to_string(draft.join(sync::BASE).join("index.md")).unwrap_or_default()));
    }
    if rev != "main" && !(rev.len() >= 7 && rev.len() <= 40 && rev.chars().all(|c| c.is_ascii_hexdigit())) {
        return Err(format!("Not a commit: {rev}"));
    }
    with_repo(app, move |_, repo, s| {
        let rev = if rev == "main" { s.base() } else { rev };
        ctx(repo, s).text_at(&rev, &slug)
    })
    .await
}


#[tauri::command]
fn get_settings(app: AppHandle) -> Result<Settings, String> {
    load_settings(&app)
}

/// Validates and saves the settings. A different repository replaces the app's clone.
#[tauri::command]
async fn save_settings(app: AppHandle, settings: Settings) -> Result<Settings, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let next = validate(&app, settings)?;
        store_settings(&app, &next)?;
        Ok(next)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Saves settings that already passed `validate`. A different repository replaces the app's clone.
fn store_settings(app: &AppHandle, next: &Settings) -> Result<(), String> {
    let state: State<AppState> = app.state();
    let prev = {
        let _guard = state.repo_lock.lock().unwrap();
        let prev = load_settings(app).unwrap_or_default();
        let repo = repo_dir(app)?;
        if prev.repo != next.repo && repo.exists() {
            fs::remove_dir_all(&repo).map_err(|e| e.to_string())?;
            *state.cache.lock().unwrap() = SyncCache::default();
            let _ = fs::remove_file(data_dir(app)?.join("sync.json"));
        }
        let json = serde_json::to_string_pretty(next).map_err(|e| e.to_string())?;
        write_atomic(&settings_path(app)?, json.as_bytes())?;
        prev
    };
    if next.backup {
        let result = (|| {
            let dir = backup_dir().ok_or_else(|| "iCloud Drive is not turned on for this Mac".to_string())?;
            fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            // When backup is turned on, settings already in the backup (from another Mac) stay so they can be restored.
            if prev.backup || !dir.join("settings.json").exists() {
                fs::copy(settings_path(app)?, dir.join("settings.json")).map_err(|e| e.to_string())?;
            }
            if !prev.backup {
                let drafts = drafts_dir(app)?;
                for slug in local_drafts(&drafts)? {
                    mirror(&drafts.join(&slug), &dir.join("drafts").join(&slug))?;
                }
            }
            Ok(())
        })();
        record_backup(app, result);
    }
    Ok(())
}

// ---------- iCloud backup ----------

/// `iCloud Drive/mdwriter`, when iCloud Drive is on. Writing there needs no entitlement; it is a normal folder.
fn backup_dir() -> Option<PathBuf> {
    let icloud = PathBuf::from(std::env::var("HOME").ok()?).join("Library/Mobile Documents/com~apple~CloudDocs");
    icloud.is_dir().then(|| icloud.join("mdwriter"))
}

fn local_drafts(drafts: &Path) -> Result<Vec<String>, String> {
    let mut slugs: Vec<String> = fs::read_dir(drafts)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().join("index.md").is_file())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|s| check_slug(s).is_ok())
        .collect();
    slugs.sort();
    Ok(slugs)
}

/// Copies the files of a draft folder that are missing or older in `to`, never deleting any, and makes the
/// sync state (`.base/` and `.mdwriter.json`) in `to` match `from` exactly, so a restored draft keeps its base.
fn mirror(from: &Path, to: &Path) -> Result<(), String> {
    fs::create_dir_all(to).map_err(|e| e.to_string())?;
    if from.join(sync::BASE).is_dir() {
        sync::replace_files(&from.join(sync::BASE), &to.join(sync::BASE))?;
    }
    if from.join(sync::META).is_file() {
        fs::copy(from.join(sync::META), to.join(sync::META)).map_err(|e| e.to_string())?;
    }
    fs::create_dir_all(to).map_err(|e| e.to_string())?;
    for name in draft_files(from)? {
        let (src, dst) = (from.join(&name), to.join(&name));
        let a = fs::metadata(&src).map_err(|e| e.to_string())?;
        let stale = match fs::metadata(&dst) {
            Ok(b) => a.len() != b.len() || a.modified().ok() > b.modified().ok(),
            Err(_) => true,
        };
        if stale {
            fs::copy(&src, &dst).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

fn record_backup(app: &AppHandle, result: Result<(), String>) {
    let state: State<AppState> = app.state();
    let mut b = state.backup.lock().unwrap();
    match result {
        Ok(()) => {
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
            *b = (Some(now.as_millis() as u64), None);
        }
        Err(e) => b.1 = Some(e),
    }
}

/// Backs up one draft when backup is on. Failures are reported by `backup_status`, not to the save.
fn backup_draft(app: &AppHandle, slug: &str) {
    if !load_settings(app).is_ok_and(|s| s.backup) {
        return;
    }
    let result = (|| {
        let dir = backup_dir().ok_or_else(|| "iCloud Drive is not turned on for this Mac".to_string())?;
        mirror(&draft_path(app, slug)?, &dir.join("drafts").join(slug))
    })();
    record_backup(app, result);
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BackupStatus {
    /// None when iCloud Drive is off.
    dir: Option<String>,
    last_backup: Option<u64>,
    error: Option<String>,
    /// Drafts in the backup that are not on this Mac.
    restorable_drafts: Vec<String>,
    /// The backup holds settings that differ from this Mac's.
    restorable_settings: bool,
}

fn backup_settings(dir: &Path) -> Option<Settings> {
    serde_json::from_str(&fs::read_to_string(dir.join("settings.json")).ok()?).ok()
}

#[tauri::command]
fn backup_status(app: AppHandle, state: State<AppState>) -> Result<BackupStatus, String> {
    let (last_backup, error) = state.backup.lock().unwrap().clone();
    let dir = backup_dir();
    let (mut restorable_drafts, mut restorable_settings) = (Vec::new(), false);
    if let Some(dir) = &dir {
        let local = drafts_dir(&app)?;
        if dir.join("drafts").is_dir() {
            restorable_drafts = local_drafts(&dir.join("drafts"))?
                .into_iter()
                .filter(|s| !local.join(s).exists())
                .collect();
        }
        if let Some(theirs) = backup_settings(dir) {
            let ours = load_settings(&app)?;
            restorable_settings = Settings { backup: ours.backup, ..theirs } != ours;
        }
    }
    Ok(BackupStatus {
        dir: dir.map(|d| d.to_string_lossy().to_string()),
        last_backup,
        error,
        restorable_drafts,
        restorable_settings,
    })
}

/// Copies drafts that are only in the backup to this Mac, then applies the backed-up settings with backup on.
/// Drafts already on this Mac are never overwritten.
#[tauri::command]
async fn restore_backup(app: AppHandle) -> Result<Settings, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let dir = backup_dir().ok_or_else(|| "iCloud Drive is not turned on for this Mac".to_string())?;
        let local = drafts_dir(&app)?;
        if dir.join("drafts").is_dir() {
            for slug in local_drafts(&dir.join("drafts"))? {
                if !local.join(&slug).exists() {
                    mirror(&dir.join("drafts").join(&slug), &local.join(&slug))?;
                }
            }
        }
        let current = load_settings(&app)?;
        let Some(theirs) = backup_settings(&dir) else { return Ok(current) };
        let next = validate(&app, Settings { backup: true, ..theirs })
            .map_err(|e| format!("Drafts were restored, but the backed-up settings were not: {e}"))?;
        store_settings(&app, &next)?;
        Ok(next)
    })
    .await
    .map_err(|e| e.to_string())?
}

// ---------- site detection ----------

/// The first `key: "value"` (any quote) in JS source where `key` is a whole word.
fn string_prop(src: &str, key: &str) -> Option<String> {
    let mut from = 0;
    while let Some(i) = src[from..].find(key).map(|i| i + from) {
        from = i + key.len();
        let word_start = src[..i].chars().next_back().map_or(true, |c| !(c.is_alphanumeric() || c == '_'));
        let rest = src[from..].trim_start();
        let Some(rest) = rest.strip_prefix(':') else { continue };
        let rest = rest.trim_start();
        let Some(q) = rest.chars().next().filter(|c| "\"'`".contains(*c)) else { continue };
        if word_start {
            return rest[1..].split(q).next().map(str::to_string);
        }
    }
    None
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SiteDetection {
    url: String,
    /// Files the URL was worked out from.
    found_in: Vec<String>,
    /// What the live check did, such as "…/blog/foo/ returned 200".
    check: String,
    verified: bool,
}

/// Works out the post URL of an Astro site from `site`/`base`/`trailingSlash` in astro.config and the
/// page in src/pages that renders the posts collection, then requests a published post to confirm it.
#[tauri::command]
async fn detect_site(app: AppHandle, branch: String, posts_dir: String) -> Result<SiteDetection, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state: State<AppState> = app.state();
        let _guard = state.repo_lock.lock().unwrap();
        let s = Settings {
            branch: branch.trim().to_string(),
            posts_dir: posts_dir.trim().trim_matches('/').to_string(),
            ..load_settings(&app)?
        };
        let repo = ensure_repo(&app, &s)?;
        git(&repo, &["fetch", "--quiet", "origin"])?;
        let rev = s.base();
        let show = |path: &str| git(&repo, &["show", &format!("{rev}:{path}")]).ok();
        let mut found_in = Vec::new();

        let (config_name, config) = ["astro.config.mjs", "astro.config.ts", "astro.config.mts", "astro.config.js", "astro.config.cjs"]
            .iter()
            .find_map(|n| show(n).map(|c| (n.to_string(), c)))
            .ok_or_else(|| format!("No astro.config file on {}, so this does not look like an Astro project", s.branch))?;
        found_in.push(config_name);
        let site = string_prop(&config, "site").ok_or("astro.config has no `site`, so the URL can't be worked out")?;
        let base = string_prop(&config, "base").unwrap_or_default();
        let trailing = string_prop(&config, "trailingSlash").unwrap_or_default();
        let format = config
            .find("build")
            .and_then(|i| config[i..].find('{').map(|j| i + j))
            .and_then(|i| config[i..].find('}').map(|j| &config[i..i + j]))
            .and_then(|block| string_prop(block, "format"))
            .unwrap_or_default();

        // The collection whose glob loader reads the posts folder; else the folder's last segment.
        let mut collection = s.posts_dir.rsplit('/').next().unwrap_or_default().to_string();
        for name in ["src/content.config.ts", "src/content.config.mjs", "src/content.config.js", "src/content/config.ts", "src/content/config.mjs", "src/content/config.js"] {
            let Some(src) = show(name) else { continue };
            if let Some(at) = src.find(&s.posts_dir) {
                if let Some(def) = src[..at].rfind("defineCollection") {
                    let before = src[..def].trim_end().trim_end_matches('=').trim_end();
                    let ident: String = before.chars().rev().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                    let ident: String = ident.chars().rev().collect();
                    if !ident.is_empty() {
                        collection = ident;
                    }
                }
            }
            found_in.push(name.to_string());
            break;
        }

        let pages = git(&repo, &["ls-tree", "-r", "--name-only", &rev, "--", "src/pages"])?;
        let page = pages
            .lines()
            .filter(|p| p.contains('[') && (p.ends_with(".astro") || p.ends_with(".ts") || p.ends_with(".js")))
            .find(|p| {
                show(p).is_some_and(|src| {
                    src.contains(&format!("getCollection(\"{collection}\""))
                        || src.contains(&format!("getCollection('{collection}'"))
                })
            })
            .ok_or_else(|| format!("No page in src/pages renders the `{collection}` collection"))?
            .to_string();
        found_in.push(page.clone());

        let route = page.strip_prefix("src/pages").unwrap_or(&page);
        let route = &route[..route.rfind('.').unwrap_or(route.len())];
        let segments: Vec<&str> = route
            .split('/')
            .map(|seg| if seg.starts_with('[') && seg.ends_with(']') { "{slug}" } else { seg })
            .collect();
        if segments.iter().filter(|seg| **seg == "{slug}").count() != 1 {
            return Err(format!("{page} has more than one route parameter, so set the Post URL by hand"));
        }
        let mut path = segments.join("/");
        if let Some(p) = path.strip_suffix("/index") {
            path = p.to_string();
        }
        let base = base.trim_matches('/');
        let base = if base.is_empty() { String::new() } else { format!("/{base}") };
        let slash = if trailing == "never" || format == "file" { "" } else { "/" };
        let url = format!("{}{base}{path}{slash}", site.trim_end_matches('/'));

        // Check against a post that is on the branch and not `draft: true` (drafts are left out of the build).
        let published = {
            let mut posts: Vec<(String, String)> = base_posts(&repo, &s)?.into_iter().collect();
            posts.sort();
            posts.into_iter().find(|(_, p)| {
                show(p).is_some_and(|src| {
                    let fm = src.split("\n---").next().unwrap_or("");
                    !fm.lines().any(|l| l.trim() == "draft: true")
                })
            })
        };
        let (check, verified) = match published {
            Some((slug, _)) => {
                let probe = url.replace("{slug}", &slug);
                let code = exec(&repo, "curl", &["-sL", "-o", "/dev/null", "-w", "%{http_code}", "--max-time", "10", &probe])
                    .unwrap_or_else(|_| "no response".into());
                (format!("{probe} returned {code}"), code == "200")
            }
            None => (format!("No published post on {} to check against", s.branch), false),
        };
        Ok(SiteDetection { url, found_in, check, verified })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Asks the window to save any pending edit, then quit. Exits anyway after 3 s if it doesn't answer.
fn request_quit(app: &AppHandle) {
    let _ = app.emit("flush-before-quit", ());
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(3));
        app.exit(0);
    });
}

/// Called by the window once its pending edit is saved.
#[tauri::command]
fn quit(app: AppHandle) {
    app.exit(0);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState::default())
        .setup(|app| {
            let cache = data_dir(app.handle())
                .ok()
                .and_then(|d| fs::read_to_string(d.join("sync.json")).ok())
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default();
            *app.state::<AppState>().cache.lock().unwrap() = cache;
            // The default Quit item terminates the app with no chance to save, so swap it for one that asks first.
            #[cfg(target_os = "macos")]
            {
                use tauri::menu::{Menu, MenuItem, MenuItemKind};
                let menu = Menu::default(app.handle())?;
                if let Some(MenuItemKind::Submenu(app_menu)) = menu.items()?.into_iter().next() {
                    let last = app_menu.items()?.len().saturating_sub(1);
                    app_menu.remove_at(last)?;
                    app_menu.append(&MenuItem::with_id(app, "quit", "Quit mdwriter", true, Some("CmdOrCtrl+Q"))?)?;
                }
                app.set_menu(menu)?;
            }
            Ok(())
        })
        .on_menu_event(|app, event| {
            if event.id() == "quit" {
                request_quit(app);
            }
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                request_quit(window.app_handle());
            }
        })
        .invoke_handler(tauri::generate_handler![
            list_posts,
            create_draft,
            read_draft,
            save_draft,
            post_files,
            add_image,
            pull_post,
            push_post,
            open_pr,
            conflict_state,
            resolve_conflict,
            abort_conflict,
            undo_pull,
            edit_post,
            post_history,
            text_at,
            get_settings,
            save_settings,
            backup_status,
            restore_backup,
            detect_site,
            quit
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

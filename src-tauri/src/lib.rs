//! Backend for mdwriter.
//!
//! Each project (one repository) has a folder in the app data dir, `projects/<id>/`, with its drafts
//! (`drafts/<slug>/index.md` plus images) and an app-owned clone (`repo/`). All git work happens in that
//! clone, never in the user's own checkout. Every action that changes a draft or the
//! remote is explicit (Pull, Push, Open PR); listing posts only reads. The git logic is in `sync.rs`.

mod sync;

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use sync::{check_slug, draft_files, exec, git, write_atomic, PrInfo};
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent};

const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "avif", "svg"];

/// One repository mdwriter writes to. Its clone, drafts and sync state live in `projects/<id>/`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Project {
    /// Folder name under `projects/`. Assigned when the project is added and never changed.
    id: String,
    /// GitHub repository as `owner/name`; cloned over SSH.
    repo: String,
    /// Branch that posts are read from and PRs target.
    branch: String,
    /// Folder in the repo that holds the posts.
    posts_dir: String,
    /// Live URL of a post, with `{slug}` in place of the slug. Empty hides the Site button.
    post_url: String,
    /// Left out of the switcher. Its clone and drafts stay on disk until it is unarchived.
    archived: bool,
}

impl Default for Project {
    fn default() -> Self {
        Self {
            id: String::new(),
            repo: String::new(),
            branch: "main".into(),
            posts_dir: "src/content/posts".into(),
            post_url: String::new(),
            archived: false,
        }
    }
}

impl Project {
    fn remote(&self) -> String {
        format!("git@github.com:{}.git", self.repo)
    }
    fn base(&self) -> String {
        format!("origin/{}", self.branch)
    }
}

/// App-wide settings and the projects, stored as `settings.json` in the app data dir.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Settings {
    editor_font_size: u32,
    /// Hide markdown syntax away from the cursor and show images inline.
    live_preview: bool,
    line_numbers: bool,
    /// Copy drafts and settings to `iCloud Drive/mdwriter` as they save.
    backup: bool,
    /// Shortcut overrides by action id, such as `"push": "Mod+Shift+P"`. An empty value turns a shortcut off.
    /// The actions and their defaults are in `src/keymap.ts`.
    keybinds: BTreeMap<String, String>,
    projects: Vec<Project>,
    /// The project the window shows; empty before setup or when every project is archived.
    active_project: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            editor_font_size: 14,
            live_preview: true,
            line_numbers: true,
            backup: false,
            keybinds: BTreeMap::new(),
            projects: Vec::new(),
            active_project: String::new(),
        }
    }
}

impl Settings {
    fn active(&self) -> Option<&Project> {
        self.projects.iter().find(|p| p.id == self.active_project && !p.archived)
    }
    /// Points `active_project` at a project that is not archived, when it doesn't already.
    fn fix_active(&mut self) {
        if self.active().is_none() {
            self.active_project = self.projects.iter().find(|p| !p.archived).map(|p| p.id.clone()).unwrap_or_default();
        }
    }
}

/// A project and its folder in the app data dir.
struct Proj {
    p: Project,
    dir: PathBuf,
}

impl std::ops::Deref for Proj {
    type Target = Project;
    fn deref(&self) -> &Project {
        &self.p
    }
}

impl Proj {
    fn repo(&self) -> PathBuf {
        self.dir.join("repo")
    }
    fn drafts(&self) -> Result<PathBuf, String> {
        let dir = self.dir.join("drafts");
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        Ok(dir)
    }
    fn draft(&self, slug: &str) -> Result<PathBuf, String> {
        check_slug(slug)?;
        Ok(self.drafts()?.join(slug))
    }
    /// Where a conflicted pull waits to be resolved.
    fn conflict(&self, slug: &str) -> Result<PathBuf, String> {
        check_slug(slug)?;
        Ok(self.dir.join("conflicts").join(slug))
    }
    /// The draft as it was before the last pull, for Undo.
    fn undo(&self, slug: &str) -> Result<PathBuf, String> {
        check_slug(slug)?;
        Ok(self.dir.join("undo").join(slug))
    }
}

#[derive(Default)]
struct AppState {
    /// Serializes every operation on the app-owned clones.
    repo_lock: Mutex<()>,
    /// Per project id: PRs from the last `gh pr list` and when that sync finished. Read lazily from the
    /// project's `sync.json`, for instant loads.
    cache: Mutex<HashMap<String, SyncCache>>,
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

// ---------- paths, settings and projects ----------

fn data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

fn project_dir(app: &AppHandle, id: &str) -> Result<PathBuf, String> {
    check_slug(id)?;
    Ok(data_dir(app)?.join("projects").join(id))
}

fn proj(app: &AppHandle, p: Project) -> Result<Proj, String> {
    let dir = project_dir(app, &p.id)?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(Proj { p, dir })
}

/// The project a command names. Archived projects are refused, so nothing writes to them.
fn project(app: &AppHandle, id: &str) -> Result<Proj, String> {
    let p = load_settings(app)?
        .projects
        .into_iter()
        .find(|p| p.id == id)
        .ok_or_else(|| format!("No project named {id:?}"))?;
    if p.archived {
        return Err(format!("{} is archived", p.repo));
    }
    proj(app, p)
}

fn settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(data_dir(app)?.join("settings.json"))
}

/// A folder name for a new project, from the repository's name, with a number added when `taken`.
fn unique_id(repo: &str, taken: impl Fn(&str) -> bool) -> String {
    let name: String = repo
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_lowercase() || c.is_ascii_digit() { c } else { '-' })
        .collect();
    let name = name.trim_matches('-');
    let name = if name.is_empty() { "project" } else { name };
    if !taken(name) {
        return name.to_string();
    }
    (2..).map(|n| format!("{name}-{n}")).find(|id| !taken(id)).unwrap()
}

fn new_id(app: &AppHandle, s: &Settings, repo: &str) -> Result<String, String> {
    let root = data_dir(app)?.join("projects");
    // A folder left on disk also counts, so a new project never picks up someone else's drafts.
    Ok(unique_id(repo, |id| s.projects.iter().any(|p| p.id == id) || root.join(id).exists()))
}

/// Before projects, `settings.json` held one repository's fields at the top level.
fn legacy_project(value: &serde_json::Value) -> Option<Project> {
    if value.get("projects").is_some() {
        return None;
    }
    let repo = value.get("repo")?.as_str().filter(|r| !r.is_empty())?;
    let get = |k: &str, d: &str| value.get(k).and_then(|v| v.as_str()).unwrap_or(d).to_string();
    Some(Project {
        repo: repo.into(),
        branch: get("branch", "main"),
        posts_dir: get("postsDir", "src/content/posts"),
        post_url: get("postUrl", ""),
        ..Project::default()
    })
}

const LEGACY_ITEMS: &[&str] = &["repo", "drafts", "conflicts", "undo", "sync.json"];

/// Moves the clone, drafts and sync state that sat at the top of the data dir before projects into the
/// project's folder. Without `force`, only when the old clone is of the project's repository.
fn adopt_legacy(app: &AppHandle, p: &Project, force: bool) -> Result<(), String> {
    let data = data_dir(app)?;
    let origin = git(&data.join("repo"), &["remote", "get-url", "origin"]).ok();
    if !force && origin.as_deref() != Some(p.remote().as_str()) {
        return Ok(());
    }
    let dir = project_dir(app, &p.id)?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    for name in LEGACY_ITEMS {
        let (from, to) = (data.join(name), dir.join(name));
        if from.exists() && !to.exists() {
            fs::rename(&from, &to).map_err(|e| format!("Couldn't move {name} into the project: {e}"))?;
        }
    }
    Ok(())
}

/// Saved settings, or the defaults when none are saved yet. Settings from before projects become one project.
fn load_settings(app: &AppHandle) -> Result<Settings, String> {
    let json = match fs::read_to_string(settings_path(app)?) {
        Ok(json) => json,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Settings::default()),
        Err(e) => return Err(e.to_string()),
    };
    let value: serde_json::Value = serde_json::from_str(&json).map_err(|e| format!("settings.json is invalid: {e}"))?;
    let legacy = legacy_project(&value);
    let mut s: Settings = serde_json::from_value(value).map_err(|e| format!("settings.json is invalid: {e}"))?;
    if let Some(mut p) = legacy {
        p.id = new_id(app, &s, &p.repo)?;
        adopt_legacy(app, &p, true)?;
        s.active_project = p.id.clone();
        s.projects.push(p);
        write_settings(app, &s)?;
    }
    Ok(s)
}

fn write_settings(app: &AppHandle, s: &Settings) -> Result<(), String> {
    let json = serde_json::to_string_pretty(s).map_err(|e| e.to_string())?;
    write_atomic(&settings_path(app)?, json.as_bytes())
}

/// Checks the app-wide settings. Projects are checked by `validate_project`.
fn validate_app(s: Settings) -> Result<Settings, String> {
    if !(10..=24).contains(&s.editor_font_size) {
        return Err("Editor font size must be between 10 and 24".into());
    }
    if s.keybinds.len() > 100 || s.keybinds.iter().any(|(k, v)| k.len() > 40 || v.len() > 40) {
        return Err("Keyboard shortcuts are invalid".into());
    }
    Ok(s)
}

/// Trims a project and rejects values that would break git, gh, or the post paths. With `check_remote`,
/// also checks that the repository and branch can be reached.
fn validate_project(app: &AppHandle, mut p: Project, check_remote: bool) -> Result<Project, String> {
    p.repo = p.repo.trim().trim_end_matches(".git").to_string();
    p.branch = p.branch.trim().to_string();
    p.posts_dir = p.posts_dir.trim().trim_matches('/').to_string();
    p.post_url = p.post_url.trim().to_string();

    let part_ok = |s: &str| {
        !s.is_empty() && !s.starts_with('.') && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
    };
    match p.repo.split_once('/') {
        Some((owner, name)) if part_ok(owner) && part_ok(name) => {}
        _ => return Err("Repository must look like owner/name".into()),
    }
    if p.branch.is_empty()
        || p.branch.starts_with('-')
        || exec(&data_dir(app)?, "git", &["check-ref-format", "--branch", &p.branch]).is_err()
    {
        return Err(format!("{:?} is not a valid branch name", p.branch));
    }
    if p.posts_dir.is_empty() || !Path::new(&p.posts_dir).components().all(|c| matches!(c, Component::Normal(_))) {
        return Err("Posts folder must be a path inside the repository".into());
    }
    if !p.post_url.is_empty()
        && !((p.post_url.starts_with("https://") || p.post_url.starts_with("http://")) && p.post_url.contains("{slug}"))
    {
        return Err("Post URL must start with https:// and contain {slug}".into());
    }
    if check_remote {
        let heads = exec(&data_dir(app)?, "git", &["ls-remote", "--heads", &p.remote(), &p.branch])
            .map_err(|_| format!("Couldn't reach {}. Check the name, and that your SSH key can access it.", p.repo))?;
        if !heads.lines().any(|l| l.ends_with(&format!("refs/heads/{}", p.branch))) {
            return Err(format!("{} has no branch named {}", p.repo, p.branch));
        }
    }
    Ok(p)
}

/// Clones the project's repository into its folder on first use.
fn ensure_repo(pj: &Proj) -> Result<PathBuf, String> {
    let repo = pj.repo();
    if !repo.join(".git").exists() {
        exec(&pj.dir, "git", &["clone", "--quiet", "--", &pj.remote(), "repo"])?;
    }
    Ok(repo)
}

/// The project's sync cache, read from its `sync.json` the first time.
fn cached(app: &AppHandle, pj: &Proj) -> SyncCache {
    let state: State<AppState> = app.state();
    let mut cache = state.cache.lock().unwrap();
    cache
        .entry(pj.id.clone())
        .or_insert_with(|| {
            fs::read_to_string(pj.dir.join("sync.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
        })
        .clone()
}

/// Changes the project's sync cache and saves it to `sync.json`.
fn update_cache(app: &AppHandle, pj: &Proj, f: impl FnOnce(&mut SyncCache)) -> Result<(), String> {
    let mut c = cached(app, pj);
    f(&mut c);
    let json = serde_json::to_string(&c).map_err(|e| e.to_string())?;
    app.state::<AppState>().cache.lock().unwrap().insert(pj.id.clone(), c);
    write_atomic(&pj.dir.join("sync.json"), json.as_bytes())
}

/// Lists the repository's PRs with gh and records them and the sync time.
fn refresh_prs(app: &AppHandle, cwd: &Path, pj: &Proj) -> Result<(), String> {
    let json = exec(
        cwd,
        "gh",
        &[
            "pr", "list", "--repo", &pj.repo, "--state", "all", "--limit", "200", "--json",
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
    update_cache(app, pj, |c| *c = SyncCache { prs, synced_at: Some(now_ms()) })
}

/// Fetches and lists PRs; what every Pull and Push starts with.
fn sync_remote(app: &AppHandle, repo: &Path, pj: &Proj) -> Result<(), String> {
    git(repo, &["fetch", "--quiet", "--prune", "origin"])?;
    refresh_prs(app, repo, pj)
}

fn prs(app: &AppHandle, pj: &Proj) -> HashMap<String, PrInfo> {
    cached(app, pj).prs
}

fn ctx<'a>(repo: &'a Path, p: &'a Project) -> sync::Ctx<'a> {
    sync::Ctx { repo, main: &p.branch, posts_dir: &p.posts_dir }
}

/// Runs `f` off the main thread with the clones locked, after making sure the project's clone exists.
async fn with_repo<T: Send + 'static>(
    app: AppHandle,
    project_id: String,
    f: impl FnOnce(&AppHandle, &Path, &Proj) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state: State<AppState> = app.state();
        let _guard = state.repo_lock.lock().unwrap();
        let pj = project(&app, &project_id)?;
        let repo = ensure_repo(&pj)?;
        f(&app, &repo, &pj)
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
fn base_posts(repo: &Path, s: &Project) -> Result<HashMap<String, String>, String> {
    let out = git(repo, &["ls-tree", "-r", "--name-only", &s.base(), "--", &s.posts_dir])?;
    Ok(out
        .lines()
        .filter_map(|p| post_id(&s.posts_dir, p).map(|id| (id, p.to_string())))
        .collect())
}



// ---------- commands ----------
//
// Every command that touches posts names its project, so a save or pull still running when the window
// switches projects lands in the project it started in.

/// Lists drafts and posts. Without `fetch` this reads only local state, so it returns at once; with it,
/// it fetches and lists PRs first. Either way it never changes a draft or the remote.
#[tauri::command]
async fn list_posts(app: AppHandle, project: String, fetch: bool) -> Result<PostList, String> {
    tauri::async_runtime::spawn_blocking(move || list_posts_blocking(&app, &project, fetch))
        .await
        .map_err(|e| e.to_string())?
}

fn list_posts_blocking(app: &AppHandle, project_id: &str, fetch: bool) -> Result<PostList, String> {
    let state: State<AppState> = app.state();
    let _guard = state.repo_lock.lock().unwrap();
    let mut warnings = Vec::new();
    let pj = project(app, project_id)?;

    let repo = match ensure_repo(&pj) {
        Ok(repo) => Some(repo),
        Err(e) => {
            warnings.push(e);
            None
        }
    };
    if let (true, Some(repo)) = (fetch, &repo) {
        if let Err(e) = sync_remote(app, repo, &pj) {
            warnings.push(e);
        }
    }
    let on_main = match &repo {
        Some(repo) => base_posts(repo, &pj).unwrap_or_else(|e| {
            warnings.push(e);
            HashMap::new()
        }),
        None => HashMap::new(),
    };
    let SyncCache { prs, synced_at } = cached(app, &pj);
    let open_pr = |slug: &str| prs.get(&format!("post/{slug}")).filter(|p| p.state == "OPEN").cloned();

    let drafts = pj.drafts()?;
    let local = local_drafts(&drafts)?;
    let local_set: HashSet<&String> = local.iter().collect();
    let mut posts = Vec::new();
    for slug in &local {
        let dir = drafts.join(slug);
        let status = repo.as_ref().map(|repo| ctx(repo, &pj).status(slug, &dir, &prs)).transpose().unwrap_or_else(|e| {
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
            conflict: pj.conflict(slug)?.join("state.json").exists(),
        });
    }
    if let Some(repo) = &repo {
        let mut remote: Vec<(&String, &String)> = on_main.iter().filter(|(id, _)| !local_set.contains(id)).collect();
        remote.sort();
        for (id, path) in remote {
            posts.push(PostSummary {
                slug: id.clone(),
                local: false,
                content: git(repo, &["show", &format!("{}:{path}", pj.base())]).unwrap_or_default(),
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
fn create_draft(app: AppHandle, state: State<AppState>, project: String, slug: String, content: String) -> Result<(), String> {
    let pj = self::project(&app, &project)?;
    let dir = pj.draft(&slug)?;
    if dir.exists() {
        return Err(format!("A draft named {slug} already exists"));
    }
    let repo = pj.repo();
    if repo.join(".git").exists() {
        let _guard = state.repo_lock.lock().unwrap();
        if base_posts(&repo, &pj)?.contains_key(&slug) {
            return Err(format!("A post with the slug {slug} is already on {}", pj.branch));
        }
    }
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    write_atomic(&dir.join("index.md"), content.as_bytes())?;
    sync::init_new(&dir)?;
    backup_draft(&app, &pj, &slug);
    Ok(())
}

#[tauri::command]
fn read_draft(app: AppHandle, project: String, slug: String) -> Result<String, String> {
    fs::read_to_string(self::project(&app, &project)?.draft(&slug)?.join("index.md")).map_err(|e| e.to_string())
}

#[tauri::command]
fn save_draft(app: AppHandle, project: String, slug: String, content: String) -> Result<(), String> {
    let pj = self::project(&app, &project)?;
    let path = pj.draft(&slug)?.join("index.md");
    if !path.exists() {
        return Err(format!("No draft named {slug}"));
    }
    write_atomic(&path, content.as_bytes())?;
    backup_draft(&app, &pj, &slug);
    Ok(())
}

#[tauri::command]
fn post_files(app: AppHandle, project: String, slug: String) -> Result<PostFiles, String> {
    let dir = self::project(&app, &project)?.draft(&slug)?;
    let images = draft_files(&dir)?
        .into_iter()
        .filter(|n| n != "index.md")
        .collect();
    Ok(PostFiles { dir: dir.to_string_lossy().to_string(), images })
}

/// Copies a dropped image beside the post's index.md and returns its file name.
#[tauri::command]
fn add_image(app: AppHandle, project: String, slug: String, source: String) -> Result<String, String> {
    let pj = self::project(&app, &project)?;
    let dir = pj.draft(&slug)?;
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
    backup_draft(&app, &pj, &slug);
    Ok(name)
}

/// Fetches, then brings remote changes into the draft. A conflict leaves the draft untouched.
#[tauri::command]
async fn pull_post(app: AppHandle, project: String, slug: String) -> Result<sync::PullOutcome, String> {
    with_repo(app, project, move |app, repo, pj| {
        sync_remote(app, repo, pj)?;
        let out = ctx(repo, pj).pull(&slug, &pj.draft(&slug)?, &prs(app, pj), &pj.conflict(&slug)?, &pj.undo(&slug)?)?;
        backup_draft(app, pj, &slug);
        Ok(out)
    })
    .await
}

/// Fetches, then commits the draft to `post/<slug>` and pushes it. Refused while the remote has changes.
#[tauri::command]
async fn push_post(app: AppHandle, project: String, slug: String) -> Result<String, String> {
    with_repo(app, project, move |app, repo, pj| {
        if pj.conflict(&slug)?.join("state.json").exists() {
            return Err("Resolve the conflict first".into());
        }
        sync_remote(app, repo, pj)?;
        let sha = ctx(repo, pj).push(&slug, &pj.draft(&slug)?, &prs(app, pj))?;
        backup_draft(app, pj, &slug);
        Ok(sha)
    })
    .await
}

/// Opens a PR from the pushed `post/<slug>` branch into the main branch.
#[tauri::command]
async fn open_pr(app: AppHandle, project: String, slug: String, title: String, body: String) -> Result<PrInfo, String> {
    with_repo(app, project, move |app, repo, pj| {
        let branch = format!("post/{slug}");
        if sync::read_meta(&pj.draft(&slug)?).branch.as_deref() != Some(branch.as_str())
            || sync::rev_sha(repo, &format!("origin/{branch}")).is_none()
        {
            return Err("Push the post first".into());
        }
        let out = exec(
            repo,
            "gh",
            &["pr", "create", "--repo", &pj.repo, "--base", &pj.branch, "--head", &branch, "--title", &title, "--body", &body],
        )?;
        let url = out.lines().rev().find(|l| l.starts_with("https://")).ok_or_else(|| format!("Unexpected gh output: {out}"))?;
        let number = url.rsplit('/').next().and_then(|n| n.parse().ok()).ok_or_else(|| format!("Unexpected gh output: {url}"))?;
        let pr = PrInfo {
            number,
            url: url.to_string(),
            state: "OPEN".into(),
            head_oid: sync::rev_sha(repo, &format!("origin/{branch}")).unwrap_or_default(),
        };
        update_cache(app, pj, |c| {
            c.prs.insert(branch, pr.clone());
        })?;
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
fn conflict_state(app: AppHandle, project: String, slug: String) -> Result<Option<ConflictView>, String> {
    let dir = self::project(&app, &project)?.conflict(&slug)?;
    Ok(sync::read_conflict(&dir)
        .map(|conflict| ConflictView { conflict, remote_dir: dir.join("remote").to_string_lossy().to_string() }))
}

/// Writes the resolved text and image choices into the draft; the remote becomes its new base.
#[tauri::command]
fn resolve_conflict(
    app: AppHandle,
    project: String,
    slug: String,
    text: Option<String>,
    remote_images: Vec<String>,
) -> Result<(), String> {
    let pj = self::project(&app, &project)?;
    sync::resolve(&pj.draft(&slug)?, &pj.conflict(&slug)?, &pj.undo(&slug)?, text.as_deref(), &remote_images)?;
    backup_draft(&app, &pj, &slug);
    Ok(())
}

/// Drops a conflicted pull. The draft was never changed, so nothing else happens.
#[tauri::command]
fn abort_conflict(app: AppHandle, project: String, slug: String) -> Result<(), String> {
    let dir = self::project(&app, &project)?.conflict(&slug)?;
    if dir.exists() {
        fs::remove_dir_all(dir).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Puts the draft back exactly as it was before the last pull or resolve.
#[tauri::command]
fn undo_pull(app: AppHandle, project: String, slug: String) -> Result<(), String> {
    let pj = self::project(&app, &project)?;
    sync::undo_pull(&pj.draft(&slug)?, &pj.undo(&slug)?)?;
    backup_draft(&app, &pj, &slug);
    Ok(())
}

/// Starts a local draft from a post on the main branch.
#[tauri::command]
async fn edit_post(app: AppHandle, project: String, slug: String) -> Result<(), String> {
    with_repo(app, project, move |app, repo, pj| {
        let draft = pj.draft(&slug)?;
        if draft.exists() {
            return Err(format!("A draft named {slug} already exists"));
        }
        let result = ctx(repo, pj).start_from_main(&slug, &draft);
        if result.is_err() {
            let _ = fs::remove_dir_all(&draft);
        }
        result?;
        backup_draft(app, pj, &slug);
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
async fn post_history(app: AppHandle, project: String, slug: String) -> Result<Vec<Commit>, String> {
    check_slug(&slug)?;
    with_repo(app, project, move |_, repo, pj| {
        let mut args = vec!["log".to_string(), "-n".into(), "30".into(), "--format=%H%x1f%ct%x1f%s".into(), pj.base()];
        let branch = format!("origin/post/{slug}");
        if sync::rev_sha(repo, &branch).is_some() {
            args.push(branch);
        }
        args.extend(["--".into(), format!("{}/{slug}", pj.posts_dir), format!("{}/{slug}.md", pj.posts_dir)]);
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
async fn text_at(app: AppHandle, project: String, slug: String, rev: String) -> Result<Option<String>, String> {
    if rev == "base" {
        let draft = self::project(&app, &project)?.draft(&slug)?;
        return Ok(sync::base_known(&draft).then(|| fs::read_to_string(draft.join(sync::BASE).join("index.md")).unwrap_or_default()));
    }
    check_slug(&slug)?;
    if rev != "main" && !(rev.len() >= 7 && rev.len() <= 40 && rev.chars().all(|c| c.is_ascii_hexdigit())) {
        return Err(format!("Not a commit: {rev}"));
    }
    with_repo(app, project, move |_, repo, pj| {
        let rev = if rev == "main" { pj.base() } else { rev };
        ctx(repo, pj).text_at(&rev, &slug)
    })
    .await
}

#[tauri::command]
fn get_settings(app: AppHandle) -> Result<Settings, String> {
    load_settings(&app)
}

/// Saves the app-wide settings and edits to existing projects. Projects are added, switched and archived
/// by their own commands, so those parts of `settings` are ignored here.
#[tauri::command]
async fn save_settings(app: AppHandle, settings: Settings) -> Result<Settings, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let incoming = validate_app(settings)?;
        let prev = load_settings(&app)?;
        let mut projects = prev.projects.clone();
        for p in &mut projects {
            let Some(new) = incoming.projects.iter().find(|q| q.id == p.id) else { continue };
            let new = Project { id: p.id.clone(), archived: p.archived, ..new.clone() };
            // Reaching the remote is only needed when it changes.
            let check = new.repo.trim().trim_end_matches(".git") != p.repo || new.branch.trim() != p.branch;
            *p = validate_project(&app, new, check)?;
        }
        let next = Settings { projects, active_project: prev.active_project.clone(), ..incoming };
        store_settings(&app, &prev, &next)?;
        Ok(next)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Checks the repository is reachable, adds it as a project and makes it the active one.
#[tauri::command]
async fn add_project(app: AppHandle, project: Project) -> Result<Settings, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let prev = load_settings(&app)?;
        let mut p = validate_project(&app, project, true)?;
        if let Some(dup) = prev.projects.iter().find(|q| q.repo == p.repo && q.branch == p.branch) {
            return Err(if dup.archived {
                format!("{} ({}) is archived. Unarchive it instead.", dup.repo, dup.branch)
            } else {
                format!("{} ({}) is already a project", dup.repo, dup.branch)
            });
        }
        p.id = new_id(&app, &prev, &p.repo)?;
        p.archived = false;
        {
            // A clone and drafts from before projects carry over to the project for the same repository.
            let state: State<AppState> = app.state();
            let _guard = state.repo_lock.lock().unwrap();
            adopt_legacy(&app, &p, false)?;
        }
        let mut next = prev.clone();
        next.active_project = p.id.clone();
        next.projects.push(p);
        store_settings(&app, &prev, &next)?;
        Ok(next)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Makes a project the one the window shows on launch.
#[tauri::command]
fn switch_project(app: AppHandle, id: String) -> Result<Settings, String> {
    let prev = load_settings(&app)?;
    if !prev.projects.iter().any(|p| p.id == id && !p.archived) {
        return Err(format!("No project named {id:?}"));
    }
    let next = Settings { active_project: id, ..prev.clone() };
    store_settings(&app, &prev, &next)?;
    Ok(next)
}

/// Archives or unarchives a project. Archiving only hides it: its clone and drafts stay on disk.
#[tauri::command]
fn archive_project(app: AppHandle, id: String, archived: bool) -> Result<Settings, String> {
    let prev = load_settings(&app)?;
    let mut next = prev.clone();
    let p = next.projects.iter_mut().find(|p| p.id == id).ok_or_else(|| format!("No project named {id:?}"))?;
    p.archived = archived;
    if !archived && next.active().is_none() {
        next.active_project = id;
    }
    next.fix_active();
    store_settings(&app, &prev, &next)?;
    Ok(next)
}

/// Saves settings that already passed validation. A project whose repository changed gets a fresh clone.
fn store_settings(app: &AppHandle, prev: &Settings, next: &Settings) -> Result<(), String> {
    let state: State<AppState> = app.state();
    {
        let _guard = state.repo_lock.lock().unwrap();
        for p in &next.projects {
            let dir = project_dir(app, &p.id)?;
            let repo = dir.join("repo");
            let origin = git(&repo, &["remote", "get-url", "origin"]).ok();
            if repo.exists() && origin.as_deref() != Some(p.remote().as_str()) {
                fs::remove_dir_all(&repo).map_err(|e| e.to_string())?;
                state.cache.lock().unwrap().remove(&p.id);
                let _ = fs::remove_file(dir.join("sync.json"));
            }
        }
        write_settings(app, next)?;
    }
    if next.backup {
        let result = (|| {
            let dir = backup_dir().ok_or_else(|| "iCloud Drive is not turned on for this Mac".to_string())?;
            fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            // When backup is turned on, settings already in the backup (from another Mac) stay so they can be restored.
            if prev.backup || !dir.join("settings.json").exists() {
                fs::copy(settings_path(app)?, dir.join("settings.json")).map_err(|e| e.to_string())?;
            }
            if !prev.backup {
                for p in &next.projects {
                    let pj = proj(app, p.clone())?;
                    let drafts = pj.drafts()?;
                    for slug in local_drafts(&drafts)? {
                        mirror(&drafts.join(&slug), &backup_drafts(&dir, &p.id).join(&slug))?;
                    }
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
/// It holds `settings.json` and `projects/<id>/drafts/<slug>/`, like the app data dir.
fn backup_dir() -> Option<PathBuf> {
    let icloud = PathBuf::from(std::env::var("HOME").ok()?).join("Library/Mobile Documents/com~apple~CloudDocs");
    icloud.is_dir().then(|| icloud.join("mdwriter"))
}

fn backup_drafts(dir: &Path, id: &str) -> PathBuf {
    dir.join("projects").join(id).join("drafts")
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
        Ok(()) => *b = (Some(now_ms()), None),
        Err(e) => b.1 = Some(e),
    }
}

/// Backs up one draft when backup is on. Failures are reported by `backup_status`, not to the save.
fn backup_draft(app: &AppHandle, pj: &Proj, slug: &str) {
    if !load_settings(app).is_ok_and(|s| s.backup) {
        return;
    }
    let result = (|| {
        let dir = backup_dir().ok_or_else(|| "iCloud Drive is not turned on for this Mac".to_string())?;
        mirror(&pj.draft(slug)?, &backup_drafts(&dir, &pj.id).join(slug))
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
    /// The backup holds settings or projects that differ from this Mac's.
    restorable_settings: bool,
}

fn backup_settings(dir: &Path) -> Option<Settings> {
    serde_json::from_str(&fs::read_to_string(dir.join("settings.json")).ok()?).ok()
}

/// How each project in the backup maps onto this Mac: the local project (matched by repository and
/// branch, or new with a free id), the backup's drafts folder for it, and whether it is new here.
fn backup_plan(app: &AppHandle, ours: &Settings, theirs: &Settings, dir: &Path) -> Result<Vec<(Project, PathBuf, bool)>, String> {
    let mut taken = ours.clone();
    let mut plan = Vec::new();
    for t in &theirs.projects {
        // The id names a folder in the backup, which another Mac wrote; never follow one that isn't a plain name.
        if check_slug(&t.id).is_err() {
            continue;
        }
        let from = backup_drafts(dir, &t.id);
        match ours.projects.iter().find(|o| o.repo == t.repo && o.branch == t.branch) {
            Some(o) => plan.push((o.clone(), from, false)),
            None => {
                let free = taken.projects.iter().all(|p| p.id != t.id) && !project_dir(app, &t.id)?.exists();
                let id = if free { t.id.clone() } else { new_id(app, &taken, &t.repo)? };
                let p = Project { id, ..t.clone() };
                taken.projects.push(p.clone());
                plan.push((p, from, true));
            }
        }
    }
    Ok(plan)
}

#[tauri::command]
fn backup_status(app: AppHandle, state: State<AppState>) -> Result<BackupStatus, String> {
    let (last_backup, error) = state.backup.lock().unwrap().clone();
    let dir = backup_dir();
    let (mut restorable_drafts, mut restorable_settings) = (Vec::new(), false);
    if let Some(theirs) = dir.as_deref().and_then(backup_settings) {
        let dir = dir.as_deref().unwrap();
        let ours = load_settings(&app)?;
        for (p, from, new) in backup_plan(&app, &ours, &theirs, dir)? {
            restorable_settings |= new;
            if from.is_dir() {
                let local = project_dir(&app, &p.id)?.join("drafts");
                restorable_drafts.extend(local_drafts(&from)?.into_iter().filter(|s| !local.join(s).exists()));
            }
        }
        let theirs_app = Settings { backup: ours.backup, projects: ours.projects.clone(), active_project: ours.active_project.clone(), ..theirs };
        restorable_settings |= theirs_app != ours;
    }
    Ok(BackupStatus {
        dir: dir.map(|d| d.to_string_lossy().to_string()),
        last_backup,
        error,
        restorable_drafts,
        restorable_settings,
    })
}

/// Copies drafts that are only in the backup to this Mac, adds the backup's projects that aren't here, and
/// applies its app-wide settings with backup on. Drafts and projects already on this Mac are never overwritten.
#[tauri::command]
async fn restore_backup(app: AppHandle) -> Result<Settings, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let dir = backup_dir().ok_or_else(|| "iCloud Drive is not turned on for this Mac".to_string())?;
        let ours = load_settings(&app)?;
        let Some(theirs) = backup_settings(&dir) else { return Ok(ours) };
        let mut projects = ours.projects.clone();
        for (p, from, new) in backup_plan(&app, &ours, &theirs, &dir)? {
            let p = if new {
                validate_project(&app, p, false).map_err(|e| format!("A backed-up project is invalid: {e}"))?
            } else {
                p
            };
            let local = proj(&app, p.clone())?.drafts()?;
            if from.is_dir() {
                for slug in local_drafts(&from)? {
                    if !local.join(&slug).exists() {
                        mirror(&from.join(&slug), &local.join(&slug))?;
                    }
                }
            }
            if new {
                projects.push(p);
            }
        }
        let mut next = validate_app(Settings { backup: true, projects, active_project: ours.active_project.clone(), ..theirs })?;
        next.fix_active();
        store_settings(&app, &ours, &next)?;
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
async fn detect_site(app: AppHandle, project: String, branch: String, posts_dir: String) -> Result<SiteDetection, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state: State<AppState> = app.state();
        let _guard = state.repo_lock.lock().unwrap();
        let pj = self::project(&app, &project)?;
        // The form's branch and folder may not be saved yet; check them the same way saving would.
        let p = validate_project(&app, Project { branch, posts_dir, ..pj.p.clone() }, false)?;
        let s = Proj { p, dir: pj.dir.clone() };
        let repo = ensure_repo(&s)?;
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
            add_project,
            switch_project,
            archive_project,
            backup_status,
            restore_backup,
            detect_site,
            quit
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_ids_come_from_the_repo_name_and_never_collide() {
        assert_eq!(unique_id("Octo-Cat/My_Blog.site", |_| false), "my-blog-site");
        assert_eq!(unique_id("octocat/___", |_| false), "project");
        let taken = ["blog", "blog-2"];
        assert_eq!(unique_id("someone/blog", |id| taken.contains(&id)), "blog-3");
        // Ids name folders, so they must pass the same check as slugs.
        assert!(check_slug(&unique_id("x/..", |_| false)).is_ok());
    }

    #[test]
    fn settings_from_before_projects_become_one_project() {
        let old = serde_json::json!({
            "repo": "octocat/blog", "branch": "trunk", "postsDir": "content/blog",
            "postUrl": "https://example.com/{slug}/", "editorFontSize": 16, "backup": true
        });
        let p = legacy_project(&old).unwrap();
        assert_eq!((p.repo.as_str(), p.branch.as_str(), p.posts_dir.as_str()), ("octocat/blog", "trunk", "content/blog"));
        assert_eq!(p.post_url, "https://example.com/{slug}/");
        // The app-wide fields still load, and the old fields are dropped on the next save.
        let s: Settings = serde_json::from_value(old).unwrap();
        assert_eq!((s.editor_font_size, s.backup), (16, true));
        assert!(!serde_json::to_string(&s).unwrap().contains("postsDir"));

        assert!(legacy_project(&serde_json::json!({ "repo": "" })).is_none());
        assert!(legacy_project(&serde_json::json!({ "repo": "a/b", "projects": [] })).is_none());
    }

    #[test]
    fn archiving_the_active_project_moves_to_another() {
        let p = |id: &str, archived| Project { id: id.into(), archived, ..Project::default() };
        let mut s = Settings { projects: vec![p("a", true), p("b", false)], active_project: "a".into(), ..Settings::default() };
        s.fix_active();
        assert_eq!(s.active_project, "b");
        s.projects[1].archived = true;
        s.fix_active();
        assert_eq!(s.active_project, "");
        assert!(s.active().is_none());
    }
}

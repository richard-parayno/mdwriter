//! Backend for mdwriter.
//!
//! Drafts live in the app data dir (`drafts/<slug>/index.md` plus images).
//! All git work happens in an app-owned clone (`repo/`), never in the user's
//! own checkout, so publishing cannot disturb whatever is checked out there.

use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, State};

const REMOTE: &str = "git@github.com:richard-parayno/blog.git";
const GH_REPO: &str = "richard-parayno/blog";
const POSTS_DIR: &str = "src/content/posts";
const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "avif", "svg"];

#[derive(Default)]
struct AppState {
    /// Serializes every operation on the app-owned clone.
    repo_lock: Mutex<()>,
    /// Open PRs by head branch, from the last `gh pr list`.
    prs: Mutex<HashMap<String, Pr>>,
}

#[derive(Clone, Serialize)]
struct Pr {
    number: u64,
    url: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PostSummary {
    slug: String,
    /// "draft" | "review" | "published" | "changed"
    status: &'static str,
    pr: Option<Pr>,
    /// True when the post exists as a local draft the app can edit.
    local: bool,
    /// True when the local draft differs from what is on its PR branch (or main).
    dirty: bool,
    content: String,
}

#[derive(Serialize)]
struct PostList {
    posts: Vec<PostSummary>,
    warnings: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PostFiles {
    dir: String,
    images: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PublishResult {
    number: u64,
    url: String,
    created: bool,
    pushed: bool,
}

#[derive(Clone, Serialize)]
struct Progress {
    step: &'static str,
    state: &'static str,
    detail: String,
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

fn check_slug(slug: &str) -> Result<(), String> {
    let ok = !slug.is_empty()
        && slug
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !slug.starts_with('-');
    if ok {
        Ok(())
    } else {
        Err(format!("Invalid slug: {slug:?}"))
    }
}

fn draft_path(app: &AppHandle, slug: &str) -> Result<PathBuf, String> {
    check_slug(slug)?;
    Ok(drafts_dir(app)?.join(slug))
}

/// GUI apps launched from Finder get a minimal PATH; make sure git and gh resolve.
fn command(program: &str, cwd: &Path) -> Command {
    let mut cmd = Command::new(program);
    let path = std::env::var("PATH").unwrap_or_default();
    cmd.current_dir(cwd)
        .env("PATH", format!("/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:{path}"))
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GH_PROMPT_DISABLED", "1");
    cmd
}

fn exec(cwd: &Path, program: &str, args: &[&str]) -> Result<String, String> {
    let out = command(program, cwd)
        .args(args)
        .output()
        .map_err(|e| format!("Could not run {program}: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
    } else {
        Err(format!(
            "`{program} {}` failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

fn git(repo: &Path, args: &[&str]) -> Result<String, String> {
    exec(repo, "git", args)
}

/// Clones the blog into the app data dir on first use.
fn ensure_repo(app: &AppHandle) -> Result<PathBuf, String> {
    let repo = repo_dir(app)?;
    if !repo.join(".git").exists() {
        exec(&data_dir(app)?, "git", &["clone", "--quiet", REMOTE, "repo"])?;
    }
    Ok(repo)
}

fn refresh_prs(state: &AppState, cwd: &Path) -> Result<(), String> {
    let json = exec(
        cwd,
        "gh",
        &[
            "pr", "list", "--repo", GH_REPO, "--state", "open", "--limit", "200", "--json",
            "number,url,headRefName",
        ],
    )?;
    let rows: Vec<serde_json::Value> = serde_json::from_str(&json).map_err(|e| e.to_string())?;
    let mut map = HashMap::new();
    for row in rows {
        if let (Some(head), Some(number), Some(url)) = (
            row["headRefName"].as_str(),
            row["number"].as_u64(),
            row["url"].as_str(),
        ) {
            map.insert(head.to_string(), Pr { number, url: url.to_string() });
        }
    }
    *state.prs.lock().unwrap() = map;
    Ok(())
}

/// Maps `src/content/posts/foo.md` and `src/content/posts/foo/index.md` to `foo`,
/// the same ID Astro's glob loader gives them.
fn post_id(path: &str) -> Option<String> {
    let rel = path.strip_prefix(POSTS_DIR)?.strip_prefix('/')?;
    let id = rel.strip_suffix(".md")?;
    Some(id.strip_suffix("/index").unwrap_or(id).to_string())
}

/// Post files on `origin/main`, as id -> path.
fn main_posts(repo: &Path) -> Result<HashMap<String, String>, String> {
    let out = git(repo, &["ls-tree", "-r", "--name-only", "origin/main", "--", POSTS_DIR])?;
    Ok(out
        .lines()
        .filter_map(|p| post_id(p).map(|id| (id, p.to_string())))
        .collect())
}

fn draft_files(dir: &Path) -> Result<Vec<String>, String> {
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

/// Whether the draft folder differs from `<rev>:src/content/posts/<slug>/`.
fn differs_from(repo: &Path, rev: &str, slug: &str, dir: &Path) -> Result<bool, String> {
    let prefix = format!("{POSTS_DIR}/{slug}/");
    let tree = git(repo, &["ls-tree", "-r", rev, "--", &prefix])?;
    let remote: HashMap<String, String> = tree
        .lines()
        .filter_map(|line| {
            let (meta, path) = line.split_once('\t')?;
            let hash = meta.split_whitespace().nth(2)?;
            Some((path.strip_prefix(&prefix)?.to_string(), hash.to_string()))
        })
        .collect();
    let files = draft_files(dir)?;
    if files.len() != remote.len() {
        return Ok(true);
    }
    let paths: Vec<String> = files.iter().map(|f| dir.join(f).to_string_lossy().to_string()).collect();
    let mut args = vec!["hash-object"];
    args.extend(paths.iter().map(String::as_str));
    let hashes = git(repo, &args)?;
    Ok(files
        .iter()
        .zip(hashes.lines())
        .any(|(name, hash)| remote.get(name).map(String::as_str) != Some(hash)))
}

fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
    fs::create_dir_all(to).map_err(|e| e.to_string())?;
    for name in draft_files(from)? {
        fs::copy(from.join(&name), to.join(&name)).map_err(|e| e.to_string())?;
    }
    Ok(())
}

// ---------- commands ----------

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

    let repo = match ensure_repo(app) {
        Ok(repo) => Some(repo),
        Err(e) => {
            warnings.push(e);
            None
        }
    };
    if fetch {
        if let Some(repo) = &repo {
            if let Err(e) = git(repo, &["fetch", "--quiet", "--prune", "origin"]) {
                warnings.push(e);
            }
            if let Err(e) = refresh_prs(&state, repo) {
                warnings.push(e);
            }
        }
    }
    let on_main = match &repo {
        Some(repo) => main_posts(repo).unwrap_or_else(|e| {
            warnings.push(e);
            HashMap::new()
        }),
        None => HashMap::new(),
    };
    let prs = state.prs.lock().unwrap().clone();

    let drafts = drafts_dir(app)?;
    let mut local: Vec<String> = fs::read_dir(&drafts)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().join("index.md").is_file())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    local.sort();
    let local_set: HashSet<&String> = local.iter().collect();

    let mut posts = Vec::new();
    for slug in &local {
        let dir = drafts.join(slug);
        let content = fs::read_to_string(dir.join("index.md")).unwrap_or_default();
        let pr = prs.get(&format!("post/{slug}")).cloned();
        let compare_to = if pr.is_some() {
            Some(format!("origin/post/{slug}"))
        } else if on_main.contains_key(slug) {
            Some("origin/main".to_string())
        } else {
            None
        };
        let dirty = match (&repo, &compare_to) {
            (Some(repo), Some(rev)) => differs_from(repo, rev, slug, &dir).unwrap_or(true),
            _ => true,
        };
        let status = match (&pr, on_main.contains_key(slug)) {
            (Some(_), _) => "review",
            (None, true) if dirty => "changed",
            (None, true) => "published",
            (None, false) => "draft",
        };
        posts.push(PostSummary { slug: slug.clone(), status, pr, local: true, dirty, content });
    }
    if let Some(repo) = &repo {
        let mut remote: Vec<(&String, &String)> =
            on_main.iter().filter(|(id, _)| !local_set.contains(id)).collect();
        remote.sort();
        for (id, path) in remote {
            let content = git(repo, &["show", &format!("origin/main:{path}")]).unwrap_or_default();
            posts.push(PostSummary {
                slug: id.clone(),
                status: "published",
                pr: prs.get(&format!("post/{id}")).cloned(),
                local: false,
                dirty: false,
                content,
            });
        }
    }
    Ok(PostList { posts, warnings })
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
        if main_posts(&repo)?.contains_key(&slug) {
            return Err(format!("A post with the slug {slug} is already on main"));
        }
    }
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    fs::write(dir.join("index.md"), content).map_err(|e| e.to_string())
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
    fs::write(path, content).map_err(|e| e.to_string())
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
    Ok(name)
}

#[tauri::command]
async fn publish(app: AppHandle, slug: String, title: String, body: String) -> Result<PublishResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut step = "sync";
        let result = publish_blocking(&app, &slug, &title, &body, &mut step);
        if let Err(e) = &result {
            let _ = app.emit("publish-progress", Progress { step, state: "error", detail: e.clone() });
        }
        result
    })
    .await
    .map_err(|e| e.to_string())?
}

fn publish_blocking(
    app: &AppHandle,
    slug: &str,
    title: &str,
    body: &str,
    step: &mut &'static str,
) -> Result<PublishResult, String> {
    let draft = draft_path(app, slug)?;
    if !draft.join("index.md").is_file() {
        return Err(format!("No draft named {slug}"));
    }
    let state: State<AppState> = app.state();
    let _guard = state.repo_lock.lock().unwrap();
    let emit = |step: &'static str, state: &'static str, detail: String| {
        let _ = app.emit("publish-progress", Progress { step, state, detail });
    };
    let branch = format!("post/{slug}");
    let dest = format!("{POSTS_DIR}/{slug}");

    *step = "sync";
    emit("sync", "active", "Fetching origin".into());
    let repo = ensure_repo(app)?;
    git(&repo, &["fetch", "--quiet", "--prune", "origin"])?;
    refresh_prs(&state, &repo)?;
    let existing = state.prs.lock().unwrap().get(&branch).cloned();
    if existing.is_none() && main_posts(&repo)?.get(slug).is_some_and(|p| !p.ends_with("/index.md")) {
        return Err(format!("{slug} is a single-file post on main; mdwriter only publishes folder posts"));
    }
    emit("sync", "done", String::new());

    *step = "branch";
    let base = match &existing {
        Some(_) => format!("origin/{branch}"),
        None => "origin/main".to_string(),
    };
    emit("branch", "active", String::new());
    git(&repo, &["checkout", "--quiet", "--force", "--no-track", "-B", &branch, &base])?;
    git(&repo, &["clean", "-fdq"])?;
    let dest_path = repo.join(&dest);
    if dest_path.exists() {
        fs::remove_dir_all(&dest_path).map_err(|e| e.to_string())?;
    }
    copy_dir(&draft, &dest_path)?;
    emit("branch", "done", String::new());

    *step = "commit";
    emit("commit", "active", String::new());
    git(&repo, &["add", "--all", "--", &dest])?;
    let staged = command("git", &repo)
        .args(["diff", "--cached", "--quiet"])
        .status()
        .map_err(|e| e.to_string())?;
    let has_changes = !staged.success();
    if has_changes {
        git(&repo, &["commit", "--quiet", "-m", &format!("post: {slug}")])?;
        emit("commit", "done", String::new());
    } else if existing.is_some() {
        emit("commit", "done", "No changes since the last push".into());
    } else {
        return Err("This post is identical to what is on main; nothing to publish".into());
    }

    *step = "push";
    if has_changes {
        emit("push", "active", String::new());
        if existing.is_some() {
            git(&repo, &["push", "--quiet", "origin", &format!("HEAD:refs/heads/{branch}")])?;
        } else {
            // A leftover branch from a closed PR is replaced; the lease refuses if it moved since the fetch.
            git(&repo, &["push", "--quiet", "--force-with-lease", "origin", &format!("HEAD:refs/heads/{branch}")])?;
        }
        emit("push", "done", String::new());
    } else {
        emit("push", "done", "Nothing to push".into());
    }

    *step = "pr";
    let result = match existing {
        Some(pr) => {
            emit("pr", "done", format!("Updated PR #{}", pr.number));
            PublishResult { number: pr.number, url: pr.url, created: false, pushed: has_changes }
        }
        None => {
            emit("pr", "active", String::new());
            let url = gh_pr_create(&repo, &branch, title, body)?;
            let number = url
                .rsplit('/')
                .next()
                .and_then(|n| n.parse().ok())
                .ok_or_else(|| format!("Unexpected gh output: {url}"))?;
            state
                .prs
                .lock()
                .unwrap()
                .insert(branch.clone(), Pr { number, url: url.clone() });
            emit("pr", "done", format!("Opened PR #{number}"));
            PublishResult { number, url, created: true, pushed: true }
        }
    };
    Ok(result)
}

fn gh_pr_create(repo: &Path, branch: &str, title: &str, body: &str) -> Result<String, String> {
    let out = exec(
        repo,
        "gh",
        &["pr", "create", "--repo", GH_REPO, "--base", "main", "--head", branch, "--title", title, "--body", body],
    )?;
    out.lines()
        .rev()
        .find(|l| l.starts_with("https://"))
        .map(str::to_string)
        .ok_or_else(|| format!("Unexpected gh output: {out}"))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            list_posts,
            create_draft,
            read_draft,
            save_draft,
            post_files,
            add_image,
            publish
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

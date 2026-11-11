//! The window's state and everything that changes it. Views read `App`; handlers call the
//! functions here. Git and network work runs on worker threads (`work`), and each result comes
//! back as a closure on a channel that `poll` drains at the start of a frame.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use rune::{Image, ImageCache, KeyEvent, TextEdit, Tx, Waker};
use rune_kit::ShortcutRecorderState;
use rune_kit::diff::{DiffMode, DiffState, FileDiff};

use crate::backend::{self, BackupStatus, Commit, PostFiles, PostSummary, Project, Settings, SiteDetection};
use crate::frontmatter::{self, Frontmatter, Post};
use crate::keymap;
use crate::linediff::{self, Chunk};
use crate::sync::{Conflict, PullOutcome};

pub type Done = Box<dyn FnOnce(&mut App, &mut Tx) + Send>;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Screen {
    Editor,
    Setup,
    Settings,
    Review,
    Conflict,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tone {
    Muted,
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SaveState {
    #[default]
    Saved,
    Saving,
    Failed,
}

#[derive(Clone, Debug)]
pub enum ToastAction {
    Undo(String),
    OpenPr(String),
    View(String),
}

#[derive(Clone, Debug)]
pub struct ToastMsg {
    pub text: String,
    pub error: bool,
    pub busy: bool,
    pub action: Option<(String, ToastAction)>,
    /// Seconds on the app clock when it goes away; None stays until closed or replaced.
    pub until: Option<f64>,
}

/// The revision the change markers and the field dots compare the draft against.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Since {
    pub rev: String,
    pub label: String,
}

/// The open post: its frontmatter fields and body as edit state.
#[derive(Default)]
pub struct Doc {
    pub extra: serde_yaml_ng::Mapping,
    pub title: TextEdit,
    pub description: TextEdit,
    pub date: TextEdit,
    pub tag: TextEdit,
    pub tags: Vec<String>,
    pub draft: bool,
    pub body: TextEdit,
    /// What is on disk.
    pub saved: String,
    pub dirty_since: Option<f64>,
    pub dirty: bool,
    pub save: SaveState,
    pub files: PostFiles,
}

#[derive(Default)]
pub struct SetupForm {
    pub adding: bool,
    pub repo: TextEdit,
    pub branch: TextEdit,
    pub posts_dir: TextEdit,
    pub post_url: TextEdit,
    /// Set once the repository step is done: the settings with the new project in them.
    pub saved: Option<Settings>,
    pub project: Option<Project>,
    pub busy: bool,
    pub detecting: bool,
    pub error: String,
    pub detected: Option<SiteDetection>,
    pub backup: Option<BackupStatus>,
}

#[derive(Default)]
pub struct Prefs {
    pub repo: TextEdit,
    pub branch: TextEdit,
    pub posts_dir: TextEdit,
    pub post_url: TextEdit,
    pub font_size: TextEdit,
    pub live_preview: bool,
    pub line_numbers: bool,
    pub backup_on: bool,
    pub keybinds: BTreeMap<String, String>,
    pub recorders: Vec<ShortcutRecorderState>,
    pub note: String,
    pub saving: bool,
    pub error: String,
    pub detecting: bool,
    pub detected: Option<SiteDetection>,
    pub detect_error: String,
    pub backup: Option<BackupStatus>,
    pub restoring: bool,
}

#[derive(Default)]
pub struct ConflictUi {
    pub slug: String,
    pub conflict: Option<Conflict>,
    pub mine: TextEdit,
    /// Image names resolved to the remote side.
    pub remote_images: Vec<String>,
    pub chunks: Vec<Chunk>,
}

#[derive(Clone)]
pub struct Cmd {
    pub group: &'static str,
    pub label: String,
    pub shortcut: String,
    pub run: Run,
}

#[derive(Clone, Debug)]
pub enum Run {
    Action(&'static str),
    UndoPull,
    SinceMain,
    Edit,
    Open(String),
    Project(String),
    AddProject,
    Goto(String),
}

#[derive(Default)]
pub struct Palette {
    pub open: bool,
    pub projects: bool,
    pub query: TextEdit,
    pub index: usize,
}

pub struct App {
    pub be: backend::AppHandle,
    inbox: Receiver<Done>,
    outbox: Sender<Done>,
    pub waker: Option<Waker>,
    /// Scenes and tests run worker jobs on the calling thread, so a snapshot shows their result.
    pub inline: bool,
    pub images: Option<ImageCache>,
    pub thumbs: HashMap<PathBuf, Image>,
    pub now: f64,
    tick: f64,

    pub settings: Settings,
    pub screen: Screen,
    pub fatal: Option<String>,
    pub posts: Vec<PostSummary>,
    pub warnings: Vec<String>,
    pub synced_at: Option<u64>,
    pub syncing: bool,
    pub loaded: bool,
    pub slug: Option<String>,
    pub doc: Doc,
    pub busy: bool,
    pub toast: Option<ToastMsg>,

    pub since: Option<Since>,
    pub since_post: Option<Post>,
    pub since_open: bool,
    pub history: Vec<Commit>,
    pub chunks: Vec<Chunk>,
    pub marker: Option<usize>,
    pub editor_w: f32,

    pub creating: bool,
    pub new_title: TextEdit,
    pub new_error: String,

    pub palette: Palette,
    pub setup: SetupForm,
    pub prefs: Prefs,
    pub review: DiffState,
    pub review_upstream: String,
    pub conflict: ConflictUi,
}

impl App {
    pub fn new(be: backend::AppHandle) -> App {
        let (outbox, inbox) = channel();
        let mut s = App {
            be,
            inbox,
            outbox,
            waker: None,
            inline: false,
            images: None,
            thumbs: HashMap::new(),
            now: 0.0,
            tick: 0.0,
            settings: Settings::default(),
            screen: Screen::Editor,
            fatal: None,
            posts: Vec::new(),
            warnings: Vec::new(),
            synced_at: None,
            syncing: false,
            loaded: false,
            slug: None,
            doc: Doc::default(),
            busy: false,
            toast: None,
            since: None,
            since_post: None,
            since_open: false,
            history: Vec::new(),
            chunks: Vec::new(),
            marker: None,
            editor_w: 0.0,
            creating: false,
            new_title: TextEdit::new(""),
            new_error: String::new(),
            palette: Palette::default(),
            setup: SetupForm::default(),
            prefs: Prefs::default(),
            review: DiffState::new(Vec::new()),
            review_upstream: String::new(),
            conflict: ConflictUi::default(),
        };
        s.review.set_mode(DiffMode::Split);
        s
    }

    pub fn project(&self) -> Option<&Project> {
        self.settings.active()
    }
    pub fn pid(&self) -> String {
        self.settings.active_project.clone()
    }
    pub fn summary(&self) -> Option<&PostSummary> {
        let slug = self.slug.as_deref()?;
        self.posts.iter().find(|p| p.slug == slug)
    }
    pub fn local(&self) -> bool {
        self.summary().is_some_and(|p| p.local)
    }
    /// The frontmatter as the fields have it now.
    pub fn fm(&self) -> Frontmatter {
        Frontmatter {
            title: self.doc.title.text.clone(),
            description: self.doc.description.text.clone(),
            pub_date: self.doc.date.text.clone(),
            tags: self.doc.tags.clone(),
            draft: self.doc.draft,
        }
    }
    pub fn raw(&self) -> String {
        frontmatter::serialize(&Post { fm: self.fm(), body: self.doc.body.text.clone(), extra: self.doc.extra.clone() })
    }
    /// Posts in sidebar order: by status, then newest first.
    pub fn sorted(&self) -> Vec<&PostSummary> {
        let mut v: Vec<(&PostSummary, u8, String)> = self
            .posts
            .iter()
            .map(|p| {
                let fm = self.fm_of(p);
                (p, status_of(p, fm.draft).2, fm.pub_date)
            })
            .collect();
        v.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| b.2.cmp(&a.2)));
        v.into_iter().map(|x| x.0).collect()
    }
    /// A post's frontmatter: the open one from its fields, the others from the list.
    pub fn fm_of(&self, p: &PostSummary) -> Frontmatter {
        if self.slug.as_deref() == Some(p.slug.as_str()) && p.local { self.fm() } else { frontmatter::parse(&p.content).fm }
    }
    pub fn title_of(&self, p: &PostSummary) -> String {
        let t = self.fm_of(p).title;
        if t.trim().is_empty() { p.slug.clone() } else { t }
    }
    pub fn site_url(&self) -> Option<String> {
        let p = self.summary()?;
        let url = &self.project()?.post_url;
        (p.on_main && !self.doc.draft && !url.is_empty()).then(|| url.replace("{slug}", &p.slug))
    }
    /// Why Push is off, or what it will do. The bool is whether it can run.
    pub fn push_state(&self) -> (bool, String) {
        let Some(p) = self.summary() else { return (false, String::new()) };
        let Some(sync) = &p.sync else { return (false, String::new()) };
        if p.conflict {
            return (false, "Resolve the conflict first".into());
        }
        if !sync.base_known {
            return (false, "Pull first: the remote version of this post is unknown here".into());
        }
        if sync.behind > 0 {
            return (false, format!("{} has changes you don't have. Pull first.", sync.upstream));
        }
        if sync.on_remote && sync.changes == 0 {
            return (false, "Nothing to push".into());
        }
        (true, format!("Commit and push to post/{}", p.slug))
    }
    pub fn can_open_pr(&self) -> bool {
        self.summary().is_some_and(|p| p.local && p.pr.is_none() && p.sync.as_ref().is_some_and(|s| s.upstream.starts_with("post/")))
    }
}

/// The status label, its tone and its sort rank.
pub fn status_of(p: &PostSummary, hidden: bool) -> (String, Tone, u8) {
    let live = if hidden { ("Hidden".to_string(), Tone::Muted, 6) } else { ("Published".to_string(), Tone::Success, 5) };
    let pr_open = p.pr.as_ref().map(|pr| (format!("PR #{} open", pr.number), Tone::Info, 4));
    if !p.local {
        return pr_open.unwrap_or(live);
    }
    if p.conflict {
        return ("Conflict".into(), Tone::Error, 0);
    }
    let Some(s) = &p.sync else { return ("Local".into(), Tone::Muted, 3) };
    if !s.base_known {
        return ("Pull needed".into(), Tone::Warning, 1);
    }
    if !s.on_remote && s.behind == 0 {
        return ("Local only".into(), Tone::Warning, 2);
    }
    let mut parts = Vec::new();
    if s.behind > 0 {
        parts.push(format!("↓ {} behind", s.behind));
    }
    if s.changes > 0 {
        parts.push(format!("{} change{}", s.changes, if s.changes == 1 { "" } else { "s" }));
    }
    if !parts.is_empty() {
        let pr = p.pr.as_ref().map(|pr| format!(" · PR #{}", pr.number)).unwrap_or_default();
        return (parts.join(" · ") + &pr, if s.behind > 0 { Tone::Info } else { Tone::Warning }, if s.behind > 0 { 1 } else { 2 });
    }
    if let Some(l) = pr_open {
        return l;
    }
    if s.upstream.starts_with("post/") {
        return ("Pushed".into(), Tone::Info, 4);
    }
    live
}

pub fn synced_label(at: Option<u64>) -> String {
    let Some(ms) = at else { return "Never synced".into() };
    let s = backend::now_ms().saturating_sub(ms) / 1000;
    match s {
        0..60 => "Synced just now".into(),
        60..3600 => format!("Synced {}m ago", (s + 30) / 60),
        3600..86400 => format!("Synced {}h ago", (s + 1800) / 3600),
        _ => format!("Synced {}", date_of(ms / 1000)),
    }
}

pub fn date_of(secs: u64) -> String {
    let (y, m, d) = frontmatter::civil(secs as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

// ---------- workers ----------

/// Runs `job` on a worker thread and `done` with its result on the UI thread.
pub fn work<T: Send + 'static>(
    s: &mut App,
    job: impl FnOnce(backend::AppHandle) -> T + Send + 'static,
    done: impl FnOnce(&mut App, T, &mut Tx) + Send + 'static,
) {
    let (be, out, waker) = (s.be.clone(), s.outbox.clone(), s.waker.clone());
    let run = move || {
        let v = job(be);
        let _ = out.send(Box::new(move |s: &mut App, tx: &mut Tx| done(s, v, tx)));
        if let Some(w) = waker {
            w();
        }
    };
    if s.inline {
        run();
    } else {
        std::thread::spawn(run);
    }
}

pub fn setup(s: &mut App, waker: Waker) {
    s.images = Some(ImageCache::new(waker.clone()));
    s.waker = Some(waker);
}

pub fn poll(s: &mut App, now: f64, tx: &mut Tx) -> bool {
    s.now = now;
    let mut changed = false;
    while let Ok(f) = s.inbox.try_recv() {
        f(s, tx);
        changed = true;
    }
    if s.doc.dirty && s.doc.dirty_since.is_none() {
        s.doc.dirty_since = Some(now);
        tx.wake_after(Duration::from_millis(500));
    }
    if s.doc.dirty && s.doc.dirty_since.is_some_and(|t| now - t >= 0.5) {
        save(s);
        refresh(s, false);
        changed = true;
    }
    if s.toast.as_ref().and_then(|t| t.until).is_some_and(|t| now >= t) {
        s.toast = None;
        changed = true;
    }
    if now - s.tick >= 30.0 {
        s.tick = now;
        tx.wake_after(Duration::from_secs(30));
        changed = true;
    }
    changed
}

// ---------- toasts ----------

pub fn toast(s: &mut App, text: impl Into<String>, action: Option<(&str, ToastAction)>) {
    let secs = if action.is_some() { 8.0 } else { 4.0 };
    s.toast = Some(ToastMsg {
        text: text.into(),
        error: false,
        busy: false,
        action: action.map(|(l, a)| (l.to_string(), a)),
        until: Some(s.now + secs),
    });
    if let Some(w) = &s.waker {
        // The frame after the deadline clears it.
        let (w, secs) = (w.clone(), secs);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs_f64(secs + 0.05));
            w();
        });
    }
}
pub fn toast_error(s: &mut App, text: impl Into<String>) {
    s.toast = Some(ToastMsg { text: text.into(), error: true, busy: false, action: None, until: None });
}
fn toast_busy(s: &mut App, text: impl Into<String>) {
    s.toast = Some(ToastMsg { text: text.into(), error: false, busy: true, action: None, until: None });
}

pub fn run_toast_action(s: &mut App, tx: &mut Tx) {
    let Some((_, action)) = s.toast.take().and_then(|t| t.action) else { return };
    match action {
        ToastAction::Undo(slug) => undo(s, slug),
        ToastAction::OpenPr(slug) => open_pr(s, slug),
        ToastAction::View(url) => open(s, &url, tx),
    }
}

pub fn open(s: &mut App, url: &str, tx: &mut Tx) {
    if let Err(e) = tx.open_url(url) {
        toast_error(s, format!("Couldn't open {url}: {e:?}"));
    }
}

// ---------- loading ----------

pub fn start(s: &mut App) {
    if let Err(e) = backend::migrate_settings(&s.be) {
        s.fatal = Some(e);
        return;
    }
    match backend::get_settings(s.be.clone()) {
        Ok(settings) => {
            s.settings = settings;
            if s.project().is_none() {
                show_setup(s, false);
            } else {
                refresh(s, false);
                refresh(s, true);
            }
        }
        Err(e) => s.fatal = Some(e),
    }
}

/// Lists the posts again; with `fetch`, from GitHub first.
pub fn refresh(s: &mut App, fetch: bool) {
    if s.project().is_none() {
        return;
    }
    let pid = s.pid();
    if fetch {
        s.syncing = true;
    }
    work(s, { let pid = pid.clone(); move |be| backend::list_posts(be, pid, fetch) }, move |s, r, tx| {
        if fetch {
            s.syncing = false;
        }
        // A list that returns after a project switch belongs to the other project.
        if s.pid() != pid {
            return;
        }
        match r {
            Ok(list) => {
                s.posts = list.posts;
                s.warnings = list.warnings;
                s.synced_at = list.synced_at;
                s.loaded = true;
                after_list(s, tx);
            }
            Err(e) => {
                s.loaded = true;
                toast_error(s, e);
            }
        }
    });
}

fn after_list(s: &mut App, tx: &mut Tx) {
    let known = s.slug.as_ref().is_some_and(|slug| s.posts.iter().any(|p| &p.slug == slug));
    if !known {
        let first = s.sorted().first().map(|p| p.slug.clone());
        s.slug = None;
        s.doc = Doc::default();
        if let Some(slug) = first {
            select(s, slug, tx);
        }
        return;
    }
    // A pull, an undo or a resolved conflict rewrote the draft underneath: show what is on disk.
    if !s.doc.dirty {
        let disk = s.summary().map(|p| p.content.clone()).unwrap_or_default();
        if disk != s.doc.saved {
            open_doc(s);
        }
    }
    let conflict = s.summary().is_some_and(|p| p.conflict);
    if conflict && s.screen == Screen::Editor {
        show_conflict(s);
    }
    if !conflict && s.screen == Screen::Conflict {
        s.screen = Screen::Editor;
    }
}

pub fn select(s: &mut App, slug: String, tx: &mut Tx) {
    flush(s);
    let local = s.posts.iter().find(|p| p.slug == slug).is_some_and(|p| p.local);
    s.slug = Some(slug);
    s.history.clear();
    s.marker = None;
    // "Since last sync" and "since main" carry over to another draft; a commit of one post doesn't.
    if !s.since.as_ref().is_some_and(|x| (x.rev == "base" || x.rev == "main") && local) {
        s.since = None;
    }
    s.since_post = None;
    if matches!(s.screen, Screen::Review | Screen::Conflict | Screen::Settings) {
        s.screen = Screen::Editor;
    }
    open_doc(s);
    load_since(s);
    if s.summary().is_some_and(|p| p.conflict) {
        show_conflict(s);
    }
    tx.blur();
}

/// Fills the fields from the selected post: the draft on disk, or the branch's text for a remote post.
pub fn open_doc(s: &mut App) {
    let Some(p) = s.summary() else { return };
    let (slug, local) = (p.slug.clone(), p.local);
    let raw = if local { backend::read_draft(s.be.clone(), s.pid(), slug.clone()).unwrap_or_else(|_| p.content.clone()) } else { p.content.clone() };
    let post = frontmatter::parse(&raw);
    let caret = s.doc.body.focus.min(post.body.len());
    let keep_caret = s.doc.body.focused && post.body.is_char_boundary(caret);
    let mut body = TextEdit::new(post.body.clone());
    body.set_caret(if keep_caret { caret } else { 0 });
    body.focused = keep_caret;
    s.doc = Doc {
        extra: post.extra,
        title: TextEdit::new(post.fm.title),
        description: TextEdit::new(post.fm.description),
        date: TextEdit::new(post.fm.pub_date),
        tag: TextEdit::new(""),
        tags: post.fm.tags,
        draft: post.fm.draft,
        body,
        saved: raw,
        dirty_since: None,
        dirty: false,
        save: SaveState::Saved,
        files: if local { backend::post_files(s.be.clone(), s.pid(), slug).unwrap_or_default() } else { PostFiles::default() },
    };
    load_thumbs(s);
    remark(s);
}

fn load_thumbs(s: &mut App) {
    let Some(cache) = &s.images else { return };
    let dir = PathBuf::from(&s.doc.files.dir);
    for name in &s.doc.files.images {
        let path = dir.join(name);
        if !s.thumbs.contains_key(&path) && let Ok(bytes) = std::fs::read(&path) {
            s.thumbs.insert(path, cache.load(bytes));
        }
    }
}

/// The image a post's `./name` reference points at, if it is loaded.
pub fn image<'a>(s: &'a App, url: &str) -> Option<&'a Image> {
    let name = url.strip_prefix("./").unwrap_or(url);
    if name.contains("://") || name.starts_with('/') || name.contains("..") {
        return None;
    }
    s.thumbs.get(&PathBuf::from(&s.doc.files.dir).join(name))
}

// ---------- editing and saving ----------

/// Called after any edit to the fields or the body.
pub fn edited(s: &mut App, tx: &mut Tx) {
    if !s.local() {
        return;
    }
    s.doc.dirty = true;
    s.doc.dirty_since = Some(tx.clock());
    s.doc.save = SaveState::Saving;
    s.marker = None;
    tx.wake_after(Duration::from_millis(500));
    remark(s);
}

/// Writes the draft now if it has unsaved edits.
pub fn flush(s: &mut App) {
    if s.doc.dirty {
        save(s);
    }
}

fn save(s: &mut App) {
    s.doc.dirty = false;
    s.doc.dirty_since = None;
    let Some(slug) = s.slug.clone() else { return };
    let raw = s.raw();
    match backend::save_draft(s.be.clone(), s.pid(), slug.clone(), raw.clone()) {
        Ok(()) => {
            s.doc.saved = raw.clone();
            s.doc.save = SaveState::Saved;
            if let Some(p) = s.posts.iter_mut().find(|p| p.slug == slug) {
                p.content = raw;
            }
        }
        Err(e) => {
            s.doc.save = SaveState::Failed;
            toast_error(s, format!("Save failed: {e}"));
        }
    }
}

pub fn commit_tag(s: &mut App, tx: &mut Tx) {
    let tag = s.doc.tag.text.trim().trim_matches(',').trim().to_string();
    s.doc.tag = focused_edit("", s.doc.tag.focused);
    if !tag.is_empty() && !s.doc.tags.contains(&tag) {
        s.doc.tags.push(tag);
        edited(s, tx);
    }
}

pub fn remove_tag(s: &mut App, i: usize, tx: &mut Tx) {
    if i < s.doc.tags.len() {
        s.doc.tags.remove(i);
        edited(s, tx);
    }
}

/// Inserts `text` as its own paragraph at the caret's line, never splitting a line.
pub fn insert_block(s: &mut App, text: &str, tx: &mut Tx) {
    let body = &s.doc.body.text;
    let at = s.doc.body.focus.min(body.len());
    let line_end = body[at..].find('\n').map_or(body.len(), |i| at + i);
    let line_start = body[..at].rfind('\n').map_or(0, |i| i + 1);
    let empty_line = body[line_start..line_end].trim().is_empty();
    let pos = if empty_line { line_start } else { line_end };
    let before = if pos == 0 || body[..pos].ends_with("\n\n") { "" } else if body[..pos].ends_with('\n') { "\n" } else { "\n\n" };
    let after = if body[pos..].starts_with("\n\n") { "" } else if body[pos..].starts_with('\n') { "\n" } else { "\n\n" };
    let insert = format!("{before}{text}{after}");
    let caret = pos + before.len() + text.len();
    let mut next = body.clone();
    next.insert_str(pos, &insert);
    s.doc.body.replace_text(next);
    s.doc.body.set_caret(caret);
    edited(s, tx);
}

pub fn add_images(s: &mut App, paths: &[PathBuf], tx: &mut Tx) {
    let Some(slug) = s.slug.clone().filter(|_| s.local() && !s.busy) else { return };
    for path in paths {
        match backend::add_image(s.be.clone(), s.pid(), slug.clone(), path.to_string_lossy().to_string()) {
            Ok(name) => {
                let alt = name.rsplit_once('.').map_or(name.as_str(), |x| x.0).replace(['-', '_'], " ");
                insert_block(s, &format!("![{alt}](./{name})"), tx);
            }
            Err(e) => toast_error(s, e),
        }
    }
    s.doc.files = backend::post_files(s.be.clone(), s.pid(), slug).unwrap_or_default();
    load_thumbs(s);
}

/// A pasted image goes through a temporary PNG, so it lands in the draft the same way a dropped file does.
pub fn paste_image(s: &mut App, png: &[u8], tx: &mut Tx) {
    let path = std::env::temp_dir().join(format!("pasted-{}.png", backend::now_ms()));
    if let Err(e) = std::fs::write(&path, png) {
        return toast_error(s, e.to_string());
    }
    add_images(s, &[path.clone()], tx);
    let _ = std::fs::remove_file(path);
}

pub fn insert_image_ref(s: &mut App, name: &str, tx: &mut Tx) {
    let alt = name.rsplit_once('.').map_or(name, |x| x.0).replace(['-', '_'], " ");
    insert_block(s, &format!("![{alt}](./{name})"), tx);
}

// ---------- change markers ----------

pub fn set_since(s: &mut App, since: Option<Since>) {
    s.since = since;
    s.since_post = None;
    s.marker = None;
    s.chunks.clear();
    load_since(s);
}

pub fn toggle_markers(s: &mut App) {
    let next = if s.since.is_some() { None } else { Some(Since { rev: "base".into(), label: "last sync".into() }) };
    set_since(s, next);
}

fn load_since(s: &mut App) {
    let (Some(since), Some(slug)) = (s.since.clone(), s.slug.clone()) else { return };
    if !s.local() {
        return;
    }
    let pid = s.pid();
    work(s, { let (slug, rev) = (slug.clone(), since.rev.clone()); move |be| backend::text_at(be, pid, slug, rev) }, move |s, r, _| {
        if s.slug.as_deref() != Some(slug.as_str()) || s.since.as_ref() != Some(&since) {
            return;
        }
        s.since_post = Some(frontmatter::parse(&r.ok().flatten().unwrap_or_default()));
        remark(s);
    });
}

pub fn load_history(s: &mut App) {
    let Some(slug) = s.slug.clone().filter(|_| s.local() && s.history.is_empty()) else { return };
    let pid = s.pid();
    work(s, { let slug = slug.clone(); move |be| backend::post_history(be, pid, slug) }, move |s, r, _| {
        if s.slug.as_deref() == Some(slug.as_str()) {
            s.history = r.unwrap_or_default();
        }
    });
}

/// Recomputes the changed line ranges between the compared revision and the body.
pub fn remark(s: &mut App) {
    s.chunks = match &s.since_post {
        Some(old) => {
            let a: Vec<&str> = old.body.split('\n').collect();
            let b: Vec<&str> = s.doc.body.text.split('\n').collect();
            linediff::chunks(&a, &b)
        }
        None => Vec::new(),
    };
}

pub fn revert_chunk(s: &mut App, i: usize, tx: &mut Tx) {
    let (Some(old), Some(chunk)) = (&s.since_post, s.chunks.get(i)) else { return };
    let a: Vec<&str> = old.body.split('\n').collect();
    let b: Vec<&str> = s.doc.body.text.split('\n').collect();
    let next = linediff::revert(&a, &b, chunk);
    let caret = s.doc.body.focus.min(next.len());
    s.doc.body.replace_text(next);
    let caret = (0..=caret).rev().find(|i| s.doc.body.text.is_char_boundary(*i)).unwrap_or(0);
    s.doc.body.set_caret(caret);
    s.marker = None;
    edited(s, tx);
}

// ---------- git actions ----------

/// Runs one git action under the busy toast. Editing is locked until it returns; then the list reloads.
fn act<T: Send + 'static>(
    s: &mut App,
    label: impl Into<String>,
    job: impl FnOnce(backend::AppHandle) -> Result<T, String> + Send + 'static,
    done: impl FnOnce(&mut App, T, &mut Tx) + Send + 'static,
) {
    if s.busy {
        return;
    }
    flush(s);
    s.busy = true;
    toast_busy(s, label);
    work(s, job, move |s, r, tx| {
        s.busy = false;
        match r {
            Ok(v) => {
                s.toast = None;
                done(s, v, tx);
            }
            Err(e) => toast_error(s, e),
        }
        refresh(s, false);
    });
}

pub fn pull(s: &mut App, slug: String) {
    let pid = s.pid();
    act(s, format!("Pulling {slug}…"), { let slug = slug.clone(); move |be| backend::pull_post(be, pid, slug) }, move |s, out, _| match out {
        PullOutcome::UpToDate => toast(s, "Already up to date", None),
        PullOutcome::Updated { files } => toast(s, format!("Pulled changes to {}", files.join(", ")), Some(("Undo", ToastAction::Undo(slug)))),
        PullOutcome::Conflict(_) => toast(s, "Pull found conflicts. Your draft is unchanged until you resolve them.", None),
    });
}

pub fn push(s: &mut App, slug: String) {
    let pid = s.pid();
    let pr = s.posts.iter().find(|p| p.slug == slug).and_then(|p| p.pr.as_ref().map(|pr| pr.number));
    act(s, format!("Pushing {slug}…"), { let slug = slug.clone(); move |be| backend::push_post(be, pid, slug) }, move |s, _sha, _| match pr {
        Some(n) => toast(s, format!("Pushed to PR #{n}"), None),
        None => toast(s, format!("Pushed post/{slug}"), Some(("Open PR", ToastAction::OpenPr(slug)))),
    });
}

pub fn open_pr(s: &mut App, slug: String) {
    let Some(project) = s.project().cloned() else { return };
    let fm = s.posts.iter().find(|p| p.slug == slug).map(|p| s.fm_of(p)).unwrap_or_default();
    let title = if fm.title.trim().is_empty() { slug.clone() } else { fm.title.clone() };
    let note = format!("Published from mdwriter. Only `{}/{}/` changes in this PR.", project.posts_dir, slug);
    let body = if fm.description.trim().is_empty() { note } else { format!("{}\n\n{}", fm.description.trim(), note) };
    let pid = s.pid();
    act(s, "Opening PR…", move |be| backend::open_pr(be, pid, slug, title, body), |s, pr, _| {
        toast(s, format!("Opened PR #{}", pr.number), Some(("View", ToastAction::View(pr.url))));
    });
}

pub fn undo(s: &mut App, slug: String) {
    let pid = s.pid();
    act(s, "Undoing pull…", move |be| backend::undo_pull(be, pid, slug), |s, (), _| {
        toast(s, "Pull undone. The draft is exactly as it was.", None);
    });
}

pub fn edit(s: &mut App, slug: String) {
    let pid = s.pid();
    let branch = s.project().map(|p| p.branch.clone()).unwrap_or_default();
    act(s, format!("Starting a draft of {slug}…"), { let slug = slug.clone(); move |be| backend::edit_post(be, pid, slug) }, move |s, (), _| {
        toast(s, format!("Editing a draft of {slug} from {branch}"), None);
    });
}

pub fn review(s: &mut App) {
    let Some(p) = s.summary().filter(|p| p.local) else { return };
    let (slug, upstream) = (p.slug.clone(), p.sync.as_ref().map(|x| x.upstream.clone()).unwrap_or_default());
    flush(s);
    let (pid, mine) = (s.pid(), s.raw());
    work(s, { let slug = slug.clone(); move |be| backend::text_at(be, pid, slug, "base".into()) }, move |s, r, _| match r {
        Ok(base) => {
            if s.slug.as_deref() != Some(slug.as_str()) {
                return;
            }
            let mut diff = DiffState::new(vec![FileDiff::from_texts("index.md", &base.unwrap_or_default(), &mine)]);
            diff.set_mode(DiffMode::Split);
            diff.set_wrap(true);
            s.review = diff;
            s.review_upstream = upstream;
            s.screen = Screen::Review;
        }
        Err(e) => toast_error(s, e),
    });
}

fn show_conflict(s: &mut App) {
    let Some(slug) = s.slug.clone() else { return };
    match backend::conflict_state(s.be.clone(), s.pid(), slug.clone()) {
        Ok(Some(view)) => {
            let mine = view.conflict.text.as_ref().map(|t| t.ours.clone()).unwrap_or_default();
            s.conflict = ConflictUi {
                slug,
                mine: TextEdit::new(mine),
                conflict: Some(view.conflict),
                remote_images: Vec::new(),
                chunks: Vec::new(),
            };
            conflict_chunks(s);
            s.screen = Screen::Conflict;
        }
        Ok(None) => {}
        Err(e) => toast_error(s, e),
    }
}

/// The line ranges where the editable side differs from the remote side.
pub fn conflict_chunks(s: &mut App) {
    let theirs = s.conflict.conflict.as_ref().and_then(|c| c.text.as_ref()).map(|t| t.theirs.clone()).unwrap_or_default();
    let a: Vec<&str> = theirs.split('\n').collect();
    let b: Vec<&str> = s.conflict.mine.text.split('\n').collect();
    s.conflict.chunks = linediff::chunks(&a, &b);
}

/// Replaces one differing range of the editable side with the remote's lines.
pub fn take_remote_chunk(s: &mut App, i: usize) {
    let Some(theirs) = s.conflict.conflict.as_ref().and_then(|c| c.text.as_ref()).map(|t| t.theirs.clone()) else { return };
    let Some(chunk) = s.conflict.chunks.get(i) else { return };
    let a: Vec<&str> = theirs.split('\n').collect();
    let b: Vec<&str> = s.conflict.mine.text.split('\n').collect();
    let next = linediff::revert(&a, &b, chunk);
    s.conflict.mine.replace_text(next);
    conflict_chunks(s);
}

pub fn take_all(s: &mut App, remote: bool) {
    let Some(t) = s.conflict.conflict.as_ref().and_then(|c| c.text.clone()) else { return };
    s.conflict.mine.replace_text(if remote { t.theirs } else { t.ours });
    conflict_chunks(s);
}

pub fn resolve(s: &mut App) {
    let slug = s.conflict.slug.clone();
    let text = s.conflict.conflict.as_ref().and_then(|c| c.text.as_ref()).map(|_| s.conflict.mine.text.clone());
    let images = s.conflict.remote_images.clone();
    let pid = s.pid();
    act(s, "Resolving…", { let slug = slug.clone(); move |be| backend::resolve_conflict(be, pid, slug, text, images) }, move |s, (), _| {
        s.screen = Screen::Editor;
        s.doc.saved.clear();
        toast(s, "Conflict resolved. Push when ready.", Some(("Undo", ToastAction::Undo(slug))));
    });
}

pub fn abort(s: &mut App) {
    let (slug, pid) = (s.conflict.slug.clone(), s.pid());
    act(s, "Aborting pull…", move |be| backend::abort_conflict(be, pid, slug), |s, (), _| {
        s.screen = Screen::Editor;
        toast(s, "Pull aborted. Your draft is unchanged.", None);
    });
}

// ---------- posts and projects ----------

pub fn start_new_post(s: &mut App, tx: &mut Tx) {
    if s.project().is_none() {
        return;
    }
    s.creating = true;
    s.new_title = TextEdit::new("");
    s.new_title.focused = true;
    s.new_error.clear();
    tx.focus("new-title", false);
}

pub fn create_post(s: &mut App, tx: &mut Tx) {
    let title = s.new_title.text.trim().to_string();
    let slug = frontmatter::slugify(&title);
    if slug.is_empty() {
        s.new_error = "Enter a title".into();
        return;
    }
    if s.posts.iter().any(|p| p.slug == slug) {
        s.new_error = format!("{slug} already exists");
        return;
    }
    let content = frontmatter::serialize(&Post {
        fm: Frontmatter { title, description: String::new(), pub_date: frontmatter::today(), tags: Vec::new(), draft: false },
        body: String::new(),
        extra: Default::default(),
    });
    flush(s);
    match backend::create_draft(s.be.clone(), s.pid(), slug.clone(), content.clone()) {
        Ok(()) => {
            s.creating = false;
            s.posts.push(PostSummary { slug: slug.clone(), local: true, content, sync: None, pr: None, on_main: false, conflict: false });
            select(s, slug, tx);
            s.doc.body.focused = true;
            tx.focus("body", false);
            refresh(s, false);
        }
        Err(e) => s.new_error = e,
    }
}

/// Applies settings a backend call returned, and reloads when the project changed.
pub fn adopt(s: &mut App, settings: Settings) {
    let changed = settings.active() != s.settings.active();
    s.settings = settings;
    if s.project().is_none() {
        s.posts.clear();
        s.slug = None;
        s.doc = Doc::default();
        show_setup(s, false);
        return;
    }
    s.screen = Screen::Editor;
    if changed {
        s.posts.clear();
        s.warnings.clear();
        s.synced_at = None;
        s.slug = None;
        s.doc = Doc::default();
        s.since = None;
        s.since_post = None;
        s.loaded = false;
        refresh(s, false);
        refresh(s, true);
    }
}

pub fn switch_project(s: &mut App, id: String) {
    if s.settings.active_project == id {
        return;
    }
    flush(s);
    match backend::switch_project(s.be.clone(), id) {
        Ok(settings) => adopt(s, settings),
        Err(e) => toast_error(s, e),
    }
}

pub fn archive(s: &mut App, id: String, archived: bool) {
    flush(s);
    work(s, move |be| backend::archive_project(be, id, archived), |s, r, _| match r {
        Ok(settings) => {
            let stay = s.screen == Screen::Settings && settings.active() == s.settings.active();
            adopt(s, settings);
            if stay {
                s.screen = Screen::Settings;
            }
        }
        Err(e) => {
            s.prefs.error = e.clone();
            s.setup.error = e;
            s.setup.busy = false;
        }
    });
}

// ---------- setup ----------

pub fn show_setup(s: &mut App, adding: bool) {
    flush(s);
    let p = Project::default();
    s.setup = SetupForm {
        adding,
        repo: focused_edit("", true),
        branch: TextEdit::new(p.branch),
        posts_dir: TextEdit::new(p.posts_dir),
        post_url: TextEdit::new(""),
        ..SetupForm::default()
    };
    s.screen = Screen::Setup;
    if !adding {
        work(s, backend::backup_status, |s, r, _| s.setup.backup = r.ok());
    }
}

pub fn setup_connect(s: &mut App) {
    if s.setup.busy || s.setup.repo.text.trim().is_empty() {
        return;
    }
    s.setup.busy = true;
    s.setup.error.clear();
    let project = Project {
        repo: s.setup.repo.text.trim().to_string(),
        branch: s.setup.branch.text.trim().to_string(),
        posts_dir: s.setup.posts_dir.text.trim().to_string(),
        ..Project::default()
    };
    // Checks the repository and branch are reachable, then adds the project and makes it active.
    work(s, move |be| backend::add_project(be, project), |s, r, _| {
        s.setup.busy = false;
        match r {
            Ok(settings) => {
                s.setup.project = settings.active().cloned();
                s.setup.saved = Some(settings);
                s.setup.post_url.focused = true;
                s.setup.repo.focused = false;
                setup_detect(s);
            }
            Err(e) => s.setup.error = e,
        }
    });
}

pub fn setup_detect(s: &mut App) {
    let Some(p) = s.setup.project.clone() else { return };
    s.setup.detecting = true;
    s.setup.detected = None;
    s.setup.error.clear();
    work(s, move |be| backend::detect_site(be, p.id, p.branch, p.posts_dir), |s, r, _| {
        s.setup.detecting = false;
        match r {
            Ok(d) => {
                s.setup.post_url.replace_text(d.url.clone());
                s.setup.detected = Some(d);
            }
            Err(e) => s.setup.error = e,
        }
    });
}

pub fn setup_finish(s: &mut App, skip: bool) {
    let (Some(mut settings), Some(project)) = (s.setup.saved.clone(), s.setup.project.clone()) else { return };
    if s.setup.busy {
        return;
    }
    s.setup.busy = true;
    let url = if skip { String::new() } else { s.setup.post_url.text.trim().to_string() };
    for p in &mut settings.projects {
        if p.id == project.id {
            p.post_url = url.clone();
        }
    }
    work(s, move |be| backend::save_settings(be, settings), |s, r, _| {
        s.setup.busy = false;
        match r {
            Ok(settings) => {
                s.settings = Settings::default();
                adopt(s, settings);
            }
            Err(e) => s.setup.error = e,
        }
    });
}

pub fn setup_cancel(s: &mut App) {
    if s.setup.adding && s.setup.saved.is_none() {
        s.screen = Screen::Editor;
    }
}

pub fn restore_backup(s: &mut App) {
    s.setup.busy = true;
    s.prefs.restoring = true;
    work(s, backend::restore_backup, |s, r, _| {
        s.setup.busy = false;
        s.prefs.restoring = false;
        match r {
            Ok(settings) => {
                s.settings = Settings::default();
                adopt(s, settings);
            }
            Err(e) => {
                s.setup.error = e.clone();
                s.prefs.error = e;
                work(s, backend::backup_status, |s, r, _| s.prefs.backup = r.ok());
            }
        }
    });
}

// ---------- settings ----------

pub fn show_settings(s: &mut App) {
    let Some(p) = s.project().cloned() else { return };
    flush(s);
    let keys = keymap::bindings(&s.settings.keybinds);
    s.prefs = Prefs {
        repo: TextEdit::new(p.repo),
        branch: TextEdit::new(p.branch),
        posts_dir: TextEdit::new(p.posts_dir),
        post_url: TextEdit::new(p.post_url),
        font_size: TextEdit::new(s.settings.editor_font_size.to_string()),
        live_preview: s.settings.live_preview,
        line_numbers: s.settings.line_numbers,
        backup_on: s.settings.backup,
        keybinds: s.settings.keybinds.clone(),
        recorders: keymap::ACTIONS.iter().map(|a| ShortcutRecorderState::new(keymap::to_shortcut(&keys[a.id]))).collect(),
        ..Prefs::default()
    };
    s.screen = Screen::Settings;
    work(s, backend::backup_status, |s, r, _| match r {
        Ok(b) => s.prefs.backup = Some(b),
        Err(e) => s.prefs.error = e,
    });
}

/// Stores a recorded shortcut for action `i`, or says why it can't be used.
pub fn set_keybind(s: &mut App, i: usize, binding: String) -> Result<(), String> {
    let action = &keymap::ACTIONS[i];
    if !binding.is_empty() {
        if let Some(why) = keymap::problem(&binding) {
            return Err(why);
        }
        let keys = keymap::bindings(&s.prefs.keybinds);
        if let Some(taken) = keymap::ACTIONS.iter().find(|a| a.id != action.id && keys[a.id] == binding) {
            return Err(format!("{} is already {}", keymap::display(&binding), taken.label));
        }
    }
    if binding == action.key {
        s.prefs.keybinds.remove(action.id);
    } else {
        s.prefs.keybinds.insert(action.id.to_string(), binding);
    }
    s.prefs.note.clear();
    s.prefs.error.clear();
    Ok(())
}

pub fn reset_keybind(s: &mut App, i: usize) {
    let action = &keymap::ACTIONS[i];
    let keys = keymap::bindings(&s.prefs.keybinds);
    if !action.key.is_empty() && let Some(taken) = keymap::ACTIONS.iter().find(|a| a.id != action.id && keys[a.id] == action.key) {
        s.prefs.note = format!("{} is now {}. Change that one first.", keymap::display(action.key), taken.label);
        return;
    }
    s.prefs.keybinds.remove(action.id);
    s.prefs.recorders[i] = ShortcutRecorderState::new(keymap::to_shortcut(action.key));
    s.prefs.note.clear();
}

pub fn prefs_detect(s: &mut App) {
    let pid = s.pid();
    let (branch, dir) = (s.prefs.branch.text.trim().to_string(), s.prefs.posts_dir.text.trim().to_string());
    s.prefs.detecting = true;
    s.prefs.detected = None;
    s.prefs.detect_error.clear();
    work(s, move |be| backend::detect_site(be, pid, branch, dir), |s, r, _| {
        s.prefs.detecting = false;
        match r {
            Ok(d) => {
                s.prefs.post_url.replace_text(d.url.clone());
                s.prefs.detected = Some(d);
            }
            Err(e) => s.prefs.detect_error = e,
        }
    });
}

pub fn prefs_save(s: &mut App) {
    if s.prefs.saving {
        return;
    }
    let mut next = s.settings.clone();
    next.editor_font_size = s.prefs.font_size.text.trim().parse().unwrap_or(0);
    next.live_preview = s.prefs.live_preview;
    next.line_numbers = s.prefs.line_numbers;
    next.backup = s.prefs.backup_on;
    next.keybinds = s.prefs.keybinds.clone();
    let pid = s.pid();
    for p in &mut next.projects {
        if p.id == pid {
            p.repo = s.prefs.repo.text.trim().to_string();
            p.branch = s.prefs.branch.text.trim().to_string();
            p.posts_dir = s.prefs.posts_dir.text.trim().to_string();
            p.post_url = s.prefs.post_url.text.trim().to_string();
        }
    }
    s.prefs.saving = true;
    s.prefs.error.clear();
    work(s, move |be| backend::save_settings(be, next), |s, r, _| {
        s.prefs.saving = false;
        match r {
            Ok(settings) => adopt(s, settings),
            Err(e) => s.prefs.error = e,
        }
    });
}

pub fn reveal_backup(s: &mut App) {
    let Some(dir) = s.prefs.backup.as_ref().and_then(|b| b.dir.clone()) else { return };
    let program = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    if let Err(e) = std::process::Command::new(program).arg(dir).spawn() {
        s.prefs.error = e.to_string();
    }
}

// ---------- commands, palette and shortcuts ----------

pub fn commands(s: &App) -> Vec<Cmd> {
    let keys = keymap::bindings(&s.settings.keybinds);
    let mut out = Vec::new();
    let mut add = |group: &'static str, label: String, run: Run| {
        let shortcut = match &run {
            Run::Action(id) => keymap::display(&keys[id]),
            Run::Project(id) if *id == s.settings.active_project => "Current".to_string(),
            _ => String::new(),
        };
        out.push(Cmd { group, label, shortcut, run });
    };
    let projects: Vec<&Project> = s.settings.projects.iter().filter(|p| !p.archived).collect();
    if !s.palette.projects {
        let branch = s.project().map(|p| p.branch.clone()).unwrap_or_default();
        if let Some(p) = s.summary() {
            if p.local && !s.busy {
                if !p.conflict {
                    add("Post", "Pull".into(), Run::Action("pull"));
                }
                if s.push_state().0 {
                    add("Post", "Push".into(), Run::Action("push"));
                }
                if s.can_open_pr() {
                    add("Post", "Open PR".into(), Run::Action("openPr"));
                }
                if p.sync.as_ref().is_some_and(|x| x.changes > 0) {
                    add("Post", "Review changes".into(), Run::Action("review"));
                }
                add("Post", "Undo last pull".into(), Run::UndoPull);
                let label = if s.since.is_some() { "Hide change markers" } else { "Show changes since last sync" };
                add("Post", label.into(), Run::Action("markers"));
                if p.on_main {
                    add("Post", format!("Show changes since {branch}"), Run::SinceMain);
                }
            }
            if !p.local && !s.busy {
                add("Post", "Edit as draft".into(), Run::Edit);
            }
            if let Some(pr) = &p.pr {
                add("Post", format!("View PR #{} on GitHub", pr.number), Run::Open(pr.url.clone()));
            }
            if let Some(url) = s.site_url() {
                add("Post", "Open on site".into(), Run::Open(url));
            }
        }
        add("App", "New post".into(), Run::Action("newPost"));
        add("App", "Sync with GitHub".into(), Run::Action("sync"));
        add("App", "Settings".into(), Run::Action("settings"));
    }
    for p in &projects {
        let twin = projects.iter().any(|q| q.id != p.id && q.repo == p.repo);
        let label = if twin { format!("Switch to {} ({})", p.repo, p.branch) } else { format!("Switch to {}", p.repo) };
        add("Project", label, Run::Project(p.id.clone()));
    }
    add("Project", "Add project…".into(), Run::AddProject);
    if !s.palette.projects {
        for p in s.sorted() {
            add("Go to", s.title_of(p), Run::Goto(p.slug.clone()));
        }
    }
    out
}

/// Whether every character of `query` appears in `label` in order, ignoring case and spaces.
pub fn matches(label: &str, query: &str) -> bool {
    let mut hay = label.chars().filter(|c| !c.is_whitespace()).flat_map(char::to_lowercase);
    query.chars().filter(|c| !c.is_whitespace()).flat_map(char::to_lowercase).all(|q| hay.any(|h| h == q))
}

pub fn filtered(s: &App) -> Vec<Cmd> {
    commands(s).into_iter().filter(|c| matches(&c.label, &s.palette.query.text)).collect()
}

pub fn open_palette(s: &mut App, projects: bool, tx: &mut Tx) {
    s.palette = Palette { open: true, projects, query: focused_edit("", true), index: 0 };
    tx.focus("palette-input", false);
}

pub fn close_palette(s: &mut App) {
    s.palette.open = false;
}

pub fn run_cmd(s: &mut App, run: Run, tx: &mut Tx) {
    s.palette.open = false;
    match run {
        Run::Action(id) => run_action(s, id, tx),
        Run::UndoPull => {
            if let Some(slug) = s.slug.clone() {
                undo(s, slug);
            }
        }
        Run::SinceMain => {
            let branch = s.project().map(|p| p.branch.clone()).unwrap_or_default();
            set_since(s, Some(Since { rev: "main".into(), label: branch }));
        }
        Run::Edit => {
            if let Some(slug) = s.slug.clone() {
                edit(s, slug);
            }
        }
        Run::Open(url) => open(s, &url, tx),
        Run::Project(id) => switch_project(s, id),
        Run::AddProject => show_setup(s, true),
        Run::Goto(slug) => select(s, slug, tx),
    }
}

pub fn run_action(s: &mut App, id: &str, tx: &mut Tx) {
    let slug = s.slug.clone().filter(|_| s.local() && !s.busy);
    match id {
        "palette" => {
            if s.palette.open && !s.palette.projects { close_palette(s) } else { open_palette(s, false, tx) }
        }
        "switchProject" => {
            if s.palette.open && s.palette.projects { close_palette(s) } else { open_palette(s, true, tx) }
        }
        "newPost" => {
            s.screen = Screen::Editor;
            start_new_post(s, tx);
        }
        "sync" => refresh(s, true),
        "settings" => show_settings(s),
        "pull" => {
            if let Some(slug) = slug.filter(|_| !s.summary().is_some_and(|p| p.conflict)) {
                pull(s, slug);
            }
        }
        "push" => {
            if let Some(slug) = slug.filter(|_| s.push_state().0) {
                push(s, slug);
            }
        }
        "openPr" => {
            if let Some(slug) = slug.filter(|_| s.can_open_pr()) {
                open_pr(s, slug);
            }
        }
        "review" => {
            if slug.is_some() && s.summary().is_some_and(|p| p.sync.as_ref().is_some_and(|x| x.changes > 0)) {
                review(s);
            }
        }
        "markers" => {
            if slug.is_some() {
                toggle_markers(s);
            }
        }
        _ => {}
    }
}

/// Window-level keys: the app's shortcuts, and Escape closing whatever is on top.
pub fn on_key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    use rune::Key;
    if s.prefs.recorders.iter().any(|r| r.is_recording()) && s.screen == Screen::Settings {
        return false;
    }
    if matches!(ev.key, Key::Escape) {
        if s.palette.open {
            close_palette(s);
        } else if s.marker.is_some() {
            s.marker = None;
        } else if s.since_open {
            s.since_open = false;
        } else if s.creating {
            s.creating = false;
        } else if s.screen == Screen::Review || s.screen == Screen::Settings {
            s.screen = Screen::Editor;
        } else if s.screen == Screen::Setup {
            setup_cancel(s);
        } else {
            return false;
        }
        return true;
    }
    if s.screen == Screen::Setup {
        return false;
    }
    if let Some(id) = keymap::action_for(&s.settings, ev) {
        run_action(s, id, tx);
        return true;
    }
    false
}

pub fn on_quit(s: &mut App, _: &mut Tx) {
    flush(s);
}

/// A text field that starts with the given text and focus. `TextEdit` implements `Drop`, so it
/// cannot be built with struct-update syntax.
pub fn focused_edit(text: &str, focused: bool) -> TextEdit {
    let mut e = TextEdit::new(text);
    e.focused = focused;
    e
}

/// The list or quote marker that opens `line`, with its indent: `"- "`, `"  3. "`, `"> "`.
fn line_marker(line: &str) -> Option<&str> {
    let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
    let rest = &line[indent..];
    let len = if rest.starts_with("- ") || rest.starts_with("* ") || rest.starts_with("+ ") || rest.starts_with("> ") {
        2
    } else {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 || digits > 9 || !(rest[digits..].starts_with(". ") || rest[digits..].starts_with(") ")) {
            return None;
        }
        digits + 2
    };
    Some(&line[..indent + len])
}

/// Handles a plain Return in a multi-line plain-text field. Rune's editor treats Return in a
/// paragraph as submit and only breaks the line on Shift-Return, so this inserts the line
/// break itself. As the old editor did, it continues a list or quote, and ends it when Return
/// is pressed on an item that has only its marker. Returns whether it handled the key.
pub fn newline(e: &mut TextEdit, ev: &KeyEvent) -> bool {
    let m = ev.mods;
    if ev.key != rune::Key::Enter || m.shift || m.alt || m.cmd || m.ctrl {
        return false;
    }
    let send = |e: &mut TextEdit, key| {
        e.key(&KeyEvent { key, mods: rune::Mods::default() }, None);
    };
    let caret = e.anchor.min(e.focus);
    let start = e.text[..caret].rfind('\n').map_or(0, |i| i + 1);
    let end = e.text[caret..].find('\n').map_or(e.text.len(), |i| caret + i);
    let marker = if e.anchor == e.focus { line_marker(&e.text[start..caret]).map(str::to_string) } else { None };
    match marker {
        // Only the marker on the line: remove it, leaving a blank line.
        Some(mk) if e.text[start..end] == mk => {
            for _ in mk.chars() {
                send(e, rune::Key::Backspace);
            }
        }
        Some(mk) => {
            let body = mk.trim_start_matches([' ', '\t']);
            let digits = body.bytes().take_while(u8::is_ascii_digit).count();
            let next = match body[..digits].parse::<u32>() {
                Ok(n) => format!("{}{}{}", &mk[..mk.len() - body.len()], n + 1, &body[digits..]),
                Err(_) => mk,
            };
            send(e, rune::Key::Text(format!("\n{next}")));
        }
        None => send(e, rune::Key::Text("\n".into())),
    }
    true
}

#[cfg(test)]
mod newline_tests {
    use super::*;

    fn press(text: &str, caret: usize) -> (String, usize) {
        let mut e = focused_edit(text, true);
        e.set_caret(caret);
        let ev = KeyEvent { key: rune::Key::Enter, mods: rune::Mods::default() };
        assert!(newline(&mut e, &ev));
        (e.text.clone(), e.focus)
    }

    #[test]
    fn return_breaks_the_line_and_continues_lists() {
        assert_eq!(press("cold frame", 4), ("cold\n frame".into(), 5));
        assert_eq!(press("- peas", 6), ("- peas\n- ".into(), 9));
        assert_eq!(press("  9. peas", 9), ("  9. peas\n  10. ".into(), 16));
        assert_eq!(press("> quote", 7), ("> quote\n> ".into(), 10));
        // A bare marker ends the list instead of adding another item.
        assert_eq!(press("- peas\n- ", 9), ("- peas\n".into(), 7));
        // A dash that is not a list marker is ordinary text.
        assert_eq!(press("-peas", 5), ("-peas\n".into(), 6));
    }

    #[test]
    fn modified_return_is_left_to_the_field() {
        let mut e = focused_edit("a", true);
        let ev = KeyEvent { key: rune::Key::Enter, mods: rune::Mods { shift: true, ..Default::default() } };
        assert!(!newline(&mut e, &ev));
    }
}

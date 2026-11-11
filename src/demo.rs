//! Fixture states for headless snapshots: `mdwriter --snap out.png --scene editor`.
//! Nothing here runs in the normal app.

use rune::TextEdit;
use rune_kit::diff::{DiffMode, DiffState, FileDiff};

use crate::app::{self, App, ConflictUi, Screen, Since};
use crate::backend::{PostSummary, Project};
use crate::frontmatter;
use crate::sync::{Conflict, PrInfo, Status, TextConflict};

pub const SCENES: &[(&str, fn(&mut App))] = &[
    ("editor", editor),
    ("source", source),
    ("markers", markers),
    ("palette", palette),
    ("readonly", readonly),
    ("setup", setup),
    ("settings", settings),
    ("review", review),
    ("conflict", conflict),
];

const OLD: &str = "---\ntitle: Notes on a slow garden\ndescription: What a year of not rushing taught me\npubDate: 2026-10-02\ntags: [garden, notes]\n---\n# A slow garden\n\nI planted **nothing** in the first month. The plan was to *watch* where the light fell before deciding anything.\n\n## What I measured\n\n- Hours of direct sun per bed\n- Where water pooled after rain\n\nThe [frost dates](https://example.com/frost/dates) mattered more than I expected.\n\n![The east bed in March](./east-bed.png)\n\nRun `make soil` before the first sowing.\n";
const NEW: &str = "---\ntitle: Notes on a slow garden\ndescription: What a year of not rushing taught me about soil\npubDate: 2026-10-02\ntags: [garden, notes, soil]\n---\n# A slow garden\n\nI planted **nothing** in the first month. The plan was to *watch* where the light fell before deciding anything, and to write it down.\n\n## What I measured\n\n- Hours of direct sun per bed\n- Where water pooled after rain\n- How long the soil stayed cold\n\nThe [frost dates](https://example.com/frost/dates) mattered more than I expected.\n\n![The east bed in March](./east-bed.png)\n";

fn status(upstream: &str, changes: u32, behind: u32, on_remote: bool) -> Option<Status> {
    Some(Status { upstream: upstream.into(), base_known: true, changes, behind, on_remote, in_sync: changes == 0 && behind == 0 })
}
fn post(slug: &str, title: &str, date: &str, local: bool, sync: Option<Status>, pr: Option<u64>, on_main: bool) -> PostSummary {
    PostSummary {
        slug: slug.into(),
        local,
        content: format!("---\ntitle: {title}\npubDate: {date}\n---\nBody of {title}.\n"),
        sync,
        pr: pr.map(|n| PrInfo { number: n, url: format!("https://github.com/octocat/blog/pull/{n}"), state: "OPEN".into(), head_oid: String::new() }),
        on_main,
        conflict: false,
    }
}

fn base(s: &mut App) {
    s.settings.projects = vec![Project { id: "blog".into(), repo: "octocat/blog".into(), post_url: "https://example.com/blog/{slug}/".into(), ..Project::default() }];
    s.settings.active_project = "blog".into();
    s.settings.line_numbers = true;
    s.settings.live_preview = true;
    let mut open = post("slow-garden", "", "", true, status("post/slow-garden", 4, 0, true), Some(42), true);
    open.content = NEW.into();
    s.posts = vec![
        open,
        post("winter-light", "Winter light", "2026-11-03", true, None, None, false),
        post("pulling-weeds", "Pulling weeds", "2026-09-14", true, status("main", 0, 2, true), None, true),
        post("compost", "Compost, eventually", "2026-08-20", false, None, Some(40), false),
        post("first-frost", "First frost", "2026-07-01", false, None, None, true),
        post("seed-catalogues", "Seed catalogues", "2026-05-11", false, None, None, true),
    ];
    s.synced_at = Some(crate::backend::now_ms() - 120_000);
    s.loaded = true;
    s.screen = Screen::Editor;
    s.slug = Some("slow-garden".into());
    app::open_doc(s);
    s.doc.files.images = vec!["east-bed.png".into()];
    s.editor_w = 760.0;
    {
        // Scenes are applied before `setup` gives the app its cache.
        let cache = rune::ImageCache::new(std::sync::Arc::new(|| {}));
        let img = cache.load(include_bytes!("../assets/demo/east-bed.png").to_vec());
        let _ = img.wait();
        s.thumbs.insert(std::path::PathBuf::from(&s.doc.files.dir).join("east-bed.png"), img);
    }
}

fn editor(s: &mut App) {
    base(s);
    let at = s.doc.body.text.find("## What").unwrap_or(0) + 4;
    s.doc.body.set_caret(at);
    s.doc.body.focused = true;
}
fn source(s: &mut App) {
    editor(s);
    s.settings.live_preview = false;
}
fn markers(s: &mut App) {
    base(s);
    s.since = Some(Since { rev: "base".into(), label: "last sync".into() });
    s.since_post = Some(frontmatter::parse(OLD));
    app::remark(s);
    s.marker = Some(0);
}
fn palette(s: &mut App) {
    base(s);
    s.palette.open = true;
    s.palette.query = app::focused_edit("pu", true);
}
fn readonly(s: &mut App) {
    base(s);
    s.slug = Some("first-frost".into());
    app::open_doc(s);
}
fn setup(s: &mut App) {
    s.screen = Screen::Setup;
    s.setup.repo = app::focused_edit("octocat/blog", true);
    s.setup.branch = TextEdit::new("main");
    s.setup.posts_dir = TextEdit::new("src/content/posts");
}
fn settings(s: &mut App) {
    base(s);
    app::show_settings(s);
}
fn review(s: &mut App) {
    base(s);
    let mut diff = DiffState::new(vec![FileDiff::from_texts("index.md", OLD, NEW)]);
    diff.set_mode(DiffMode::Split);
    diff.set_wrap(true);
    s.review = diff;
    s.review_upstream = "post/slow-garden".into();
    s.screen = Screen::Review;
}
fn conflict(s: &mut App) {
    base(s);
    s.posts[0].conflict = true;
    let ours = frontmatter::parse(NEW).body;
    let theirs = ours.replace("to write it down", "to photograph it").replace("- How long the soil stayed cold\n", "- How long the soil stayed cold\n- Which beds the cat preferred\n");
    s.conflict = ConflictUi {
        slug: "slow-garden".into(),
        mine: TextEdit::new(ours.clone()),
        conflict: Some(Conflict { upstream: "post/slow-garden".into(), text: Some(TextConflict { name: "index.md".into(), ours, theirs }), binary: vec!["east-bed.png".into()] }),
        ..ConflictUi::default()
    };
    app::conflict_chunks(s);
    s.screen = Screen::Conflict;
}

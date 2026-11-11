//! The screens that replace the editor: first-run setup, settings, review and conflict.

use rune::{Color, Cursor, El, ScrollSpec, ShortcutError, Span, TextEdit, TextStyle, col, el, row, spacer, text, text_input};
use rune_kit::diff::DiffView;
use rune_kit::{ButtonVariant, Kit, ShortcutRecorder, Switch};

use crate::app::{self, App, Screen};
use crate::backend::SiteDetection;
use crate::keymap;
use crate::ui::{btn, field, label};

fn h1(k: &Kit, t: &str) -> El {
    text(t.to_string()).font(TextStyle { size: 20.0, weight: 650.0, ..k.ty.ui }).color(k.c.text_base)
}
fn h2(k: &Kit, t: &str) -> El {
    text(t.to_string()).font(k.ty.ui_medium).color(k.c.text_base)
}
fn note(k: &Kit, t: impl Into<String>) -> El {
    text(t).font(k.ty.small).color(k.c.text_muted).wrap()
}
fn error(k: &Kit, t: &str) -> El {
    text(t.to_string()).font(k.ty.small).color(k.c.danger.fg).wrap().grow(1.0)
}
/// A labelled field with a hint under it.
fn labelled(k: &Kit, name: &str, hint: &str, input: El) -> El {
    col().w_full().gap(5.0)
        .child(text(name.to_string()).font(k.ty.small_medium).color(k.c.text_base))
        .child(input.w_full())
        .child(note(k, hint))
}
fn toggle(k: &Kit, name: &str, hint: &str, on: bool, f: fn(&mut App)) -> El {
    row().w_full().gap(12.0).items_start()
        .child(col().grow(1.0).gap(3.0).child(text(name.to_string()).font(k.ty.small_medium).color(k.c.text_base)).child(note(k, hint)))
        .child(Switch::new(on).label(name).hide_label().on_toggle(f).build(k))
}
fn detection(k: &Kit, d: &SiteDetection) -> El {
    let from = if d.found_in.is_empty() { String::new() } else { format!("Read from {}. ", d.found_in.join(", ")) };
    text(format!("{}{from}{}", if d.verified { "✓ " } else { "" }, d.check)).font(k.ty.small)
        .color(if d.verified { k.c.success.fg } else { k.c.warning.fg }).wrap()
}
fn banner(k: &Kit, msg: String, action: El) -> El {
    row().w_full().px(12.0).py(8.0).gap(10.0).center_y().radius(8.0).fill(k.c.layer_01).border(1.0, k.c.border_muted)
        .child(text(msg).font(k.ty.small).color(k.c.text_base).wrap().grow(1.0))
        .child(action)
}
fn submit(e: El, f: fn(&mut App)) -> El {
    e.on_submit(move |s: &mut App, _| f(s))
}

// ---------- setup ----------

pub fn setup(s: &App, k: &Kit, inset: f32) -> El {
    let f = &s.setup;
    let site = f.saved.is_some();
    let step = |t: &str, on: bool| text(t.to_string()).font(k.ty.small_medium).color(if on { k.c.text_base } else { k.c.text_faint });
    let mut c = col().w_full().max_w(520.0).gap(16.0)
        .child(row().gap(14.0).child(step("1 · Repository", !site)).child(step("2 · Site", site)));
    if !site {
        c = c.child(h1(k, if f.adding { "Add a project" } else { "Connect your site's repository" }))
            .child(note(k, if f.adding {
                "Each project is one repository, with its own drafts. Switch between them from the sidebar."
            } else {
                "mdwriter writes posts as Markdown in a GitHub repository and sends each one as its own pull request. It works in its own clone, so your checkout is never touched."
            }));
        if !f.adding && f.backup.as_ref().is_some_and(|b| b.restorable_settings) {
            c = c.child(banner(k, "Your iCloud backup has projects and settings from another Mac.".into(),
                btn(k, "Restore", ButtonVariant::Outline, !f.busy, "", |s, _| app::restore_backup(s))));
        }
        for p in s.settings.projects.iter().filter(|p| p.archived) {
            let id = p.id.clone();
            c = c.child(banner(k, format!("{} ({}) is archived.", p.repo, p.branch),
                btn(k, "Unarchive", ButtonVariant::Outline, !f.busy, "", move |s, _| app::archive(s, id.clone(), false))));
        }
        c = c
            .child(labelled(k, "GitHub repository", "owner/name. Cloned over SSH, so your SSH key needs access.",
                submit(field(k, "setup-repo", &f.repo, "octocat/blog", !f.busy, |s| &mut s.setup.repo), app::setup_connect)))
            .child(labelled(k, "Branch", "Posts are read from this branch, and pull requests target it.",
                submit(field(k, "setup-branch", &f.branch, "", !f.busy, |s| &mut s.setup.branch), app::setup_connect)))
            .child(labelled(k, "Posts folder", "Where posts live in the repository. The default fits most Astro sites.",
                submit(field(k, "setup-dir", &f.posts_dir, "", !f.busy, |s| &mut s.setup.posts_dir), app::setup_connect)));
        let mut actions = row().w_full().gap(8.0).center_y().child(error(k, &f.error));
        if f.adding {
            actions = actions.child(btn(k, "Cancel", ButtonVariant::Outline, !f.busy, "", |s, _| app::setup_cancel(s)));
        }
        c = c.child(actions.child(btn(k, if f.busy { "Connecting…" } else { "Connect" }, ButtonVariant::Contrast,
            !f.busy && !f.repo.text.trim().is_empty(), "", |s, _| app::setup_connect(s))));
    } else {
        c = c.child(h1(k, "Where do posts go live?"))
            .child(note(k, "The Site button opens a published post at this URL. For an Astro project, mdwriter reads it from the project and checks it on the live site."));
        let url = submit(field(k, "setup-url", &f.post_url, "https://example.com/blog/{slug}/", !f.busy, |s| &mut s.setup.post_url), |s| app::setup_finish(s, false));
        c = c.child(row().w_full().gap(8.0).child(url.grow(1.0))
            .child(btn(k, if f.detecting { "Detecting…" } else { "Detect" }, ButtonVariant::Outline, !f.detecting, "", |s, _| app::setup_detect(s))));
        if f.detecting {
            c = c.child(note(k, "Cloning the repository and reading the Astro project…"));
        } else if let Some(d) = &f.detected {
            c = c.child(detection(k, d));
        }
        c = c.child(note(k, "{slug} is replaced by each post's slug."))
            .child(row().w_full().gap(8.0).center_y()
                .child(error(k, &f.error))
                .child(btn(k, "Skip", ButtonVariant::Outline, !f.busy, "", |s, _| app::setup_finish(s, true)))
                .child(btn(k, "Done", ButtonVariant::Contrast, !f.busy && !f.detecting && !f.post_url.text.trim().is_empty(), "", |s, _| app::setup_finish(s, false))));
    }
    col().w_full().h_full().fill(k.c.bg_base)
        .child(el().w_full().h(if inset > 0.0 { 40.0 } else { 28.0 }).window_drag())
        .child(col().w_full().grow(1.0).min_h(0.0).scroll(ScrollSpec { stick_bottom: false }).key("setup")
            .child(col().w_full().px(32.0).py(32.0).items_center().child(c)))
}

// ---------- settings ----------

fn ago(ms: u64) -> String {
    let secs = crate::backend::now_ms().saturating_sub(ms) / 1000;
    match secs {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", secs / 60),
        3600..86400 => format!("{} h ago", secs / 3600),
        _ => app::date_of(ms / 1000),
    }
}

pub fn settings(s: &App, k: &Kit, _layer: &mut Vec<El>) -> El {
    let f = &s.prefs;
    let Some(active) = s.project() else { return col() };
    let section = |t: &str| col().w_full().gap(12.0).pt(8.0).child(h2(k, t));
    let id = active.id.clone();

    let project = col().w_full().gap(12.0)
        .child(row().w_full().center_y().child(h2(k, "Project").grow(1.0))
            .child(btn(k, "Archive", ButtonVariant::Ghost, true,
                "Hide this project from the switcher. Its clone and drafts stay on this Mac, and you can unarchive it here.",
                move |s, _| app::archive(s, id.clone(), true))))
        .child(labelled(k, "GitHub repository", "owner/name. mdwriter clones it over SSH into its own folder.", field(k, "pref-repo", &f.repo, "", true, |s| &mut s.prefs.repo)))
        .child(labelled(k, "Branch", "Posts are read from this branch, and new PRs target it.", field(k, "pref-branch", &f.branch, "", true, |s| &mut s.prefs.branch)))
        .child(labelled(k, "Posts folder", "Where posts live in the repository. Each post is published to <folder>/<slug>/.", field(k, "pref-dir", &f.posts_dir, "", true, |s| &mut s.prefs.posts_dir)));

    let mut site = section("Site").child(row().w_full().gap(8.0)
        .child(field(k, "pref-url", &f.post_url, "https://example.com/blog/{slug}/", true, |s| &mut s.prefs.post_url).grow(1.0))
        .child(btn(k, if f.detecting { "Detecting…" } else { "Detect" }, ButtonVariant::Outline, !f.detecting,
            "Read the Astro project on the branch and check the URL on the live site", |s, _| app::prefs_detect(s))));
    if let Some(d) = &f.detected {
        site = site.child(detection(k, d));
    }
    if !f.detect_error.is_empty() {
        site = site.child(error(k, &f.detect_error));
    }
    site = site.child(note(k, "The live URL of a post; {slug} is replaced by its slug. Leave empty to hide the Site button."));

    let editor = section("Editor")
        .child(labelled(k, "Font size", "In points, 10–24.", el().child(field(k, "pref-size", &f.font_size, "", true, |s| &mut s.prefs.font_size).w(90.0))))
        .child(toggle(k, "Live preview", "Hide markdown symbols except on the line you're editing, and style headings and links. The file stays plain markdown.",
            f.live_preview, |s| s.prefs.live_preview = !s.prefs.live_preview))
        .child(toggle(k, "Line numbers", "Number the lines of the post body, in the editor and in diffs.", f.line_numbers, |s| s.prefs.line_numbers = !s.prefs.line_numbers));

    let mut keys = section("Keyboard shortcuts");
    for (i, a) in keymap::ACTIONS.iter().enumerate() {
        let Some(state) = f.recorders.get(i) else { continue };
        let recorder = ShortcutRecorder::new(format!("key-{}", a.id), state, move |s: &mut App| &mut s.prefs.recorders[i])
            .label(a.label)
            .width(180.0)
            .on_change(move |s: &mut App, sc, _| {
                let r = app::set_keybind(s, i, sc.map(keymap::from_shortcut).unwrap_or_default());
                if let Err(e) = &r {
                    s.prefs.note = e.clone();
                }
                r.map_err(ShortcutError::Backend)
            })
            .build(k);
        let reset = if a.key.is_empty() { "Reset to none".to_string() } else { format!("Reset to {}", keymap::display(a.key)) };
        keys = keys.child(row().w_full().gap(8.0).center_y()
            .child(text(a.label).font(k.ty.ui).color(k.c.text_base).grow(1.0))
            .child(recorder)
            .child(btn(k, "Reset", ButtonVariant::Ghost, f.keybinds.contains_key(a.id), &reset, move |s, _| app::reset_keybind(s, i))));
    }
    keys = keys.child(if f.note.is_empty() {
        note(k, "Click a shortcut and press keys to change it. Backspace clears it, Escape cancels.")
    } else {
        text(f.note.clone()).font(k.ty.small).color(k.c.warning.fg).wrap()
    });

    let mut backup = section("Backup").child(toggle(k, "Back up to iCloud Drive",
        "Copies every draft (with its images) and these settings to iCloud Drive/mdwriter as they save. Your other Macs can restore from it. Nothing in the backup is ever deleted.",
        f.backup_on, |s| s.prefs.backup_on = !s.prefs.backup_on));
    if let Some(b) = &f.backup {
        if b.dir.is_none() {
            backup = backup.child(note(k, "iCloud Drive is off on this Mac."));
        } else if s.settings.backup {
            let state = match (&b.error, b.last_backup) {
                (Some(e), _) => text(format!("Last backup failed: {e}")).font(k.ty.small).color(k.c.danger.fg).wrap(),
                (None, Some(at)) => note(k, format!("Backed up {}", ago(at))),
                (None, None) => note(k, "On. Drafts back up as they save."),
            };
            backup = backup.child(row().w_full().gap(8.0).center_y().child(state.grow(1.0))
                .child(btn(k, if cfg!(target_os = "macos") { "Show in Finder" } else { "Show folder" }, ButtonVariant::Ghost, true, "", |s, _| app::reveal_backup(s))));
        }
        if !b.restorable_drafts.is_empty() || b.restorable_settings {
            let n = b.restorable_drafts.len();
            let mut has = Vec::new();
            if n > 0 {
                has.push(format!("{n} draft{} that {} not on this Mac", if n == 1 { "" } else { "s" }, if n == 1 { "is" } else { "are" }));
            }
            if b.restorable_settings {
                has.push("different settings".to_string());
            }
            backup = backup.child(banner(k, format!("The iCloud backup has {}. Restoring keeps every draft already here.", has.join(" and ")),
                btn(k, if f.restoring { "Restoring…" } else { "Restore" }, ButtonVariant::Outline, !f.restoring, "", |s, _| app::restore_backup(s))));
        }
    }

    let mut page = col().w_full().max_w(620.0).gap(18.0).child(project).child(site).child(editor).child(keys).child(backup);
    let archived: Vec<_> = s.settings.projects.iter().filter(|p| p.archived).collect();
    if !archived.is_empty() {
        let mut sec = section("Archived projects");
        for p in archived {
            let id = p.id.clone();
            sec = sec.child(row().w_full().gap(8.0).center_y()
                .child(text(format!("{} ({})", p.repo, p.branch)).font(k.ty.ui).color(k.c.text_base).grow(1.0))
                .child(btn(k, "Unarchive", ButtonVariant::Outline, true, "", move |s, _| app::archive(s, id.clone(), false))));
        }
        page = page.child(sec);
    }
    col().w_full().min_h(0.0)
        .child(col().w_full().grow(1.0).min_h(0.0).scroll(ScrollSpec { stick_bottom: false }).key("settings")
            .child(col().w_full().px(28.0).py(22.0).child(page)))
        .child(row().w_full().h(48.0).px(16.0).gap(8.0).center_y().border_sides([1.0, 0.0, 0.0, 0.0], k.c.border_muted)
            .child(error(k, &f.error))
            .child(btn(k, "Close", ButtonVariant::Outline, true, "", |s, _| s.screen = Screen::Editor))
            .child(btn(k, if f.saving { "Saving…" } else { "Save" }, ButtonVariant::Contrast, !f.saving, "", |s, _| app::prefs_save(s))))
}

// ---------- review ----------

fn page_head(k: &Kit, strong: String, rest: String) -> El {
    row().w_full().px(16.0).py(10.0).gap(8.0).center_y().border_sides([0.0, 0.0, 1.0, 0.0], k.c.border_muted)
        .child(text(strong).font(k.ty.ui_medium).color(k.c.text_base))
        .child(text(rest).font(k.ty.ui).color(k.c.text_muted).wrap().grow(1.0))
}

/// What Push would send: the last synced text against the draft, in Rune's read-only `DiffView`.
pub fn review(s: &App, k: &Kit) -> El {
    let diff = DiffView::new(|s: &App| &s.review, |s: &mut App| &mut s.review).build(s, k);
    let side = |t: String| text(t).font(k.ty.small).color(k.c.text_muted).grow(1.0);
    col().w_full().min_h(0.0)
        .child(page_head(k, "Review changes".into(), "· what Push sends to GitHub".into())
            .child(btn(k, "Close", ButtonVariant::Outline, true, "", |s, _| s.screen = Screen::Editor)))
        .child(row().w_full().px(16.0).py(6.0).child(side(format!("{} (last sync)", s.review_upstream))).child(side("Your draft".into())))
        .child(col().w_full().grow(1.0).min_h(0.0).scroll(ScrollSpec { stick_bottom: false }).key("review").child(diff))
}

// ---------- conflict ----------

/// Spans that tint whole lines, for the lines of `ranges` in `t`.
fn line_tints(t: &str, ranges: &[std::ops::Range<usize>], c: Color) -> Vec<Span> {
    let mut out = Vec::new();
    let mut off = 0;
    for (i, l) in t.split('\n').enumerate() {
        if !l.is_empty() && ranges.iter().any(|r| r.contains(&i)) {
            let mut sp = Span::new(off, off + l.len());
            sp.bg = Some(c);
            out.push(sp);
        }
        off += l.len() + 1;
    }
    out
}

/// A pull that clashed. `DiffView` is read-only, so this is two plain columns: the left one is
/// an editable field that becomes the draft, the right one the remote text. Lines that differ
/// are tinted on both sides, and each differing range has a button that takes the remote's lines.
pub fn conflict(s: &App, k: &Kit) -> El {
    let c = &s.conflict;
    let Some(conflict) = &c.conflict else { return col() };
    let free = !s.busy;
    let mono = TextStyle { size: 13.0, line_height: 20.0, ..k.ty.mono };
    let mut page = col().w_full().min_h(0.0)
        .child(page_head(k, format!("Pull found conflicts with {}.", conflict.upstream),
            "Your draft hasn't changed. Remote changes that didn't clash are already merged into the left side.".into())
            .child(btn(k, "Abort pull", ButtonVariant::Outline, free, "", |s, _| app::abort(s)))
            .child(btn(k, "Resolve", ButtonVariant::Contrast, free, "Replace the draft with the left side", |s, _| app::resolve(s))));

    if let Some(t) = &conflict.text {
        let link = |l: &str, remote: bool| text(l.to_string()).font(k.ty.small).color(k.c.text_accent).cursor(Cursor::Pointer)
            .key(format!("take-all-{remote}")).on_press(move |s: &mut App| app::take_all(s, remote));
        let head = |a: &str, b: String, l: El| row().grow(1.0).w_frac(0.5).shrink().gap(6.0).center_y()
            .child(text(a.to_string()).font(k.ty.small_medium).color(k.c.text_base)).child(label(k, b)).child(spacer()).child(l);
        page = page.child(row().w_full().px(16.0).py(8.0).gap(16.0)
            .child(head("Yours", "· editable, this becomes the draft".into(), link("Keep all mine", false)))
            .child(head("Remote", format!("· {}", conflict.upstream), link("Take all remote", true))));

        let mut chips = row().w_full().px(16.0).pb(8.0).gap(6.0).center_y().flex_wrap();
        chips = chips.child(label(k, if c.chunks.is_empty() { "The left side matches the remote.".to_string() } else { format!("{} difference{}:", c.chunks.len(), if c.chunks.len() == 1 { "" } else { "s" }) }));
        for (i, ch) in c.chunks.iter().enumerate() {
            let at = if ch.b.is_empty() { format!("after line {}", ch.b.start) } else if ch.b.len() == 1 { format!("line {}", ch.b.start + 1) } else { format!("lines {}–{}", ch.b.start + 1, ch.b.end) };
            chips = chips.child(btn(k, &format!("Take remote, {at}"), ButtonVariant::Outline, free, "Replace these lines with the remote's", move |s, _| app::take_remote_chunk(s, i)));
        }
        page = page.child(chips);

        let mine_ranges: Vec<_> = c.chunks.iter().map(|x| x.b.clone()).collect();
        let their_ranges: Vec<_> = c.chunks.iter().map(|x| x.a.clone()).collect();
        let mine = text_input(&c.mine, mono).key("conflict-mine").w_full().wrap().color(k.c.text_base)
            .spans(line_tints(&c.mine.text, &mine_ranges, k.c.success.fg.alpha(0.16)))
            .read_only(!free)
            .on_edit(|s: &mut App| -> &mut TextEdit { &mut s.conflict.mine })
            .on_input_change(|s: &mut App, _| app::conflict_chunks(s))
            .on_input_key(|s: &mut App, ev, _| {
                let done = app::newline(&mut s.conflict.mine, ev);
                if done {
                    app::conflict_chunks(s);
                }
                done
            });
        let theirs = text(t.theirs.clone()).font(mono).color(k.c.text_base).wrap().selectable(true)
            .spans(line_tints(&t.theirs, &their_ranges, k.c.info.fg.alpha(0.16)));
        let pane = |e: El| col().grow(1.0).w_frac(0.5).shrink().padding(12.0).radius(8.0).fill(k.c.layer_01).border(1.0, k.c.border_muted).child(e);
        page = page.child(col().w_full().grow(1.0).min_h(0.0).scroll(ScrollSpec { stick_bottom: false }).key("conflict")
            .child(row().w_full().px(16.0).pb(16.0).gap(16.0).items_start().child(pane(mine)).child(pane(theirs))));
    } else {
        page = page.child(spacer());
    }

    if !conflict.binary.is_empty() {
        let mut imgs = col().w_full().px(16.0).py(10.0).gap(6.0).border_sides([1.0, 0.0, 0.0, 0.0], k.c.border_muted)
            .child(label(k, "Images changed on both sides"));
        for name in &conflict.binary {
            let remote = c.remote_images.contains(name);
            let pick = |l: &str, to_remote: bool| {
                let n = name.clone();
                btn(k, l, if remote == to_remote { ButtonVariant::Contrast } else { ButtonVariant::Outline }, free, "", move |s, _| {
                    s.conflict.remote_images.retain(|x| x != &n);
                    if to_remote {
                        s.conflict.remote_images.push(n.clone());
                    }
                })
            };
            imgs = imgs.child(row().w_full().gap(8.0).center_y()
                .child(text(name.clone()).font(k.ty.mono).color(k.c.text_base).grow(1.0))
                .child(pick("Mine", false))
                .child(pick("Remote", true)));
        }
        page = page.child(imgs);
    }
    page
}

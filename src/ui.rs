//! The window: sidebar, post fields, the markdown editor with its gutter, the footer, and the
//! overlays (command palette, marker popup, toast). Rune calls `view` after every state change;
//! it returns the whole element tree and Rune works out what moved.

use rune::{Color, Cursor, Cx, El, Key, ScrollSpec, Span, TextStyle, Tx, col, el, row, spacer, text, text_input};
use rune_kit::{Button, ButtonVariant, Checkbox, ControlSize, Kit, Placement, Select, Spinner, Toast, ToastRegion, popover};

use crate::app::{self, App, SaveState, Screen, Since, Tone};
use crate::backend::{IMAGE_EXTS, PostSummary};
use crate::frontmatter::Frontmatter;
use crate::{keymap, md, pages};

pub const SIDEBAR: f32 = 236.0;

pub fn tone(k: &Kit, t: Tone) -> Color {
    match t {
        Tone::Muted => k.c.text_muted,
        Tone::Info => k.c.info.fg,
        Tone::Success => k.c.success.fg,
        Tone::Warning => k.c.warning.fg,
        Tone::Error => k.c.danger.fg,
    }
}

/// A button with a label, an optional tooltip and a handler.
pub fn btn(k: &Kit, label: &str, variant: ButtonVariant, on: bool, help: &str, f: impl Fn(&mut App, &mut Tx) + 'static) -> El {
    let mut b = Button::new(label).variant(variant).control_size(ControlSize::Regular).disabled(!on);
    if !help.is_empty() {
        b = b.help(help);
    }
    b.on_press_tx(f).build(k)
}

/// "Pull (⇧⌘L)": a tooltip with the action's shortcut, when it has one.
pub fn hint(s: &App, label: &str, action: &str) -> String {
    match keymap::bindings(&s.settings.keybinds).get(action) {
        Some(b) if !b.is_empty() => format!("{label} ({})", keymap::display(b)),
        _ => label.to_string(),
    }
}

pub fn label(k: &Kit, t: impl Into<String>) -> El {
    text(t).font(k.ty.small).color(k.c.text_muted)
}

/// A single-line field drawn like the kit's inputs.
pub fn field(k: &Kit, key: &str, edit: &rune::TextEdit, placeholder: &str, on: bool, get: fn(&mut App) -> &mut rune::TextEdit) -> El {
    let input = text_input(edit, k.ty.ui)
        .key(key.to_string())
        .single_line()
        .grow(1.0)
        .color(if on { k.c.text_base } else { k.c.text_muted })
        .placeholder(placeholder.to_string(), k.c.text_faint)
        .read_only(!on)
        .on_edit(get);
    row().h(30.0).px(9.0).center_y().radius(6.0).fill(k.c.bg_base).border(1.0, k.c.border_base).child(input)
}

pub fn view(s: &App, cx: &mut Cx<App>) -> El {
    let k = Kit::neutral(cx.system_dark());
    let inset = cx.titlebar_inset();
    let mut layer: Vec<El> = Vec::new();
    let base = if let Some(e) = &s.fatal {
        col().w_full().h_full().center().gap(8.0).padding(40.0)
            .child(text("mdwriter couldn't start").font(k.ty.ui_medium).color(k.c.text_base))
            .child(text(e.clone()).font(k.ty.ui).color(k.c.danger.fg).wrap())
    } else {
        match s.screen {
            Screen::Setup => pages::setup(s, &k, inset),
            Screen::Settings => shell(s, &k, inset, "Settings", pages::settings(s, &k, &mut layer)),
            Screen::Review => shell(s, &k, inset, "", pages::review(s, &k)),
            Screen::Conflict => shell(s, &k, inset, "", pages::conflict(s, &k)),
            Screen::Editor => shell(s, &k, inset, "", editor(s, &k, &mut layer)),
        }
    };
    palette(s, &k, &mut layer);
    if let Some(t) = &s.toast {
        let mut toast = Toast::new("toast", t.text.clone());
        if t.busy {
            toast = toast.lead(Spinner::new().build(&k));
        } else {
            toast = toast.closable().on_close(|s: &mut App| s.toast = None);
        }
        if t.error {
            toast = toast.lead(el().size(8.0, 8.0).radius(4.0).fill(k.c.danger.fg));
        }
        if let Some((l, _)) = &t.action {
            toast = toast.action(&k, l, true, app::run_toast_action);
        }
        layer.push(ToastRegion::new().toast(toast.build(&k)).build(&k));
    }
    rune_kit::root(base.fill(k.c.page), layer)
}

/// Sidebar on the left, a title bar and `main` on the right.
fn shell(s: &App, k: &Kit, inset: f32, title: &str, main: El) -> El {
    let title = if !title.is_empty() {
        title.to_string()
    } else if s.summary().is_some() {
        let t = s.doc.title.text.trim();
        if t.is_empty() { "Untitled".to_string() } else { t.to_string() }
    } else {
        "mdwriter".to_string()
    };
    let mut bar = row().w_full().h(44.0).px(16.0).gap(8.0).center_y().window_drag()
        .child(text(title).font(k.ty.ui_medium).color(k.c.text_base).truncate().grow(1.0));
    if s.screen != Screen::Settings {
        if let Some(pr) = s.summary().and_then(|p| p.pr.as_ref()) {
            let url = pr.url.clone();
            bar = bar.child(btn(k, &format!("PR #{}", pr.number), ButtonVariant::Ghost, true, &pr.url, move |s, tx| app::open(s, &url, tx)));
        }
        if let Some(site) = s.site_url() {
            let url = site.clone();
            bar = bar.child(btn(k, "Site", ButtonVariant::Ghost, true, &site, move |s, tx| app::open(s, &url, tx)));
        }
    }
    row().w_full().h_full()
        .child(sidebar(s, k, inset))
        .child(el().w(1.0).h_full().fill(k.c.border_muted))
        .child(col().grow(1.0).h_full().fill(k.c.bg_base)
            .child(bar)
            .child(el().w_full().h(1.0).fill(k.c.border_muted))
            .child(main.grow(1.0)))
}

fn sidebar(s: &App, k: &Kit, inset: f32) -> El {
    let project = s.project();
    let repo = project.map(|p| p.repo.clone()).unwrap_or_default();
    let twin = project.is_some_and(|p| s.settings.projects.iter().filter(|x| x.repo == p.repo).count() > 1);
    let mut name = row().grow(1.0).h(30.0).px(8.0).gap(6.0).center_y().radius(6.0).hover(k.c.hover).cursor(Cursor::Pointer)
        .help(hint(s, "Switch project", "switchProject"))
        .child(text(repo).font(k.ty.ui_medium).color(k.c.text_base).truncate().grow(1.0));
    if twin {
        name = name.child(label(k, project.map(|p| p.branch.clone()).unwrap_or_default()));
    }
    let head = row().w_full().gap(4.0).center_y()
        .child(name.key("project").on_press_tx(|s: &mut App, tx| app::open_palette(s, true, tx)))
        .child(btn(k, "+", ButtonVariant::Ghost, true, &hint(s, "New post", "newPost"), |s, tx| {
            if s.creating { s.creating = false } else { app::start_new_post(s, tx) }
        }))
        .child(btn(k, "⚙", ButtonVariant::Ghost, true, &hint(s, "Settings", "settings"), |s, _| {
            if s.screen == Screen::Settings { s.screen = Screen::Editor } else { app::show_settings(s) }
        }));

    let mut top = col().w_full().px(10.0).pt(if inset > 0.0 { 40.0 } else { 10.0 }).pb(8.0).gap(8.0).window_drag().child(head);
    if s.creating {
        let under = if !s.new_error.is_empty() {
            s.new_error.clone()
        } else if !s.new_title.text.trim().is_empty() {
            format!("posts/{}/index.md", crate::frontmatter::slugify(&s.new_title.text))
        } else {
            "Enter to create".to_string()
        };
        let input = text_input(&s.new_title, k.ty.ui).key("new-title").single_line().grow(1.0).autofocus()
            .color(k.c.text_base).placeholder("Post title", k.c.text_faint)
            .on_edit(|s: &mut App| &mut s.new_title)
            .on_input_change(|s: &mut App, _| s.new_error.clear())
            .on_submit(app::create_post);
        top = top
            .child(row().h(30.0).px(9.0).center_y().radius(6.0).fill(k.c.bg_base).border(1.0, k.c.text_accent).child(input))
            .child(text(under).font(k.ty.small).color(if s.new_error.is_empty() { k.c.text_muted } else { k.c.danger.fg }).truncate());
    }

    let mut list = col().w_full().px(8.0).pb(8.0).gap(2.0);
    for p in s.sorted() {
        list = list.child(post_row(s, k, p));
    }
    if s.posts.is_empty() {
        let msg = if s.syncing { "Cloning and syncing…" } else if s.loaded { "No posts yet" } else { "Loading posts…" };
        list = list.child(el().px(8.0).py(6.0).child(label(k, msg)));
    }

    let mut side = col().w(SIDEBAR).h_full().fill(k.c.layer_01)
        .child(top)
        .child(col().w_full().grow(1.0).min_h(0.0).scroll(ScrollSpec { stick_bottom: false }).key("posts").child(list));
    if !s.warnings.is_empty() {
        side = side.child(el().w_full().px(14.0).py(6.0).help(s.warnings.join("\n"))
            .child(text(format!("⚠ {}", s.warnings[0])).font(k.ty.small).color(k.c.warning.fg).truncate()));
    }
    let foot = row().w_full().h(34.0).px(14.0).center_y().hover(k.c.hover).cursor(Cursor::Pointer)
        .border_sides([1.0, 0.0, 0.0, 0.0], k.c.border_muted)
        .help(hint(s, "Fetch from GitHub. Only updates the counts; drafts never change.", "sync"))
        .child(label(k, if s.syncing { "Syncing…".to_string() } else { app::synced_label(s.synced_at) }))
        .key("sync")
        .on_press(|s: &mut App| if !s.syncing { app::refresh(s, true) });
    side.child(foot)
}

fn post_row(s: &App, k: &Kit, p: &PostSummary) -> El {
    let fm = s.fm_of(p);
    let (status, t, _) = app::status_of(p, fm.draft);
    let selected = s.slug.as_deref() == Some(p.slug.as_str());
    let slug = p.slug.clone();
    let title = if selected && p.local && fm.title.trim().is_empty() { "Untitled".to_string() } else { s.title_of(p) };
    let mut r = col().key(format!("post-{}", p.slug)).w_full().px(8.0).py(7.0).gap(2.0).radius(6.0).cursor(Cursor::Pointer)
        .child(text(title).font(k.ty.ui_medium).color(k.c.text_base).truncate())
        .child(row().w_full().gap(6.0)
            .child(text(status).font(k.ty.small).color(tone(k, t)).truncate().grow(1.0))
            .child(label(k, fm.pub_date.clone())))
        .on_press_tx(move |s: &mut App, tx| app::select(s, slug.clone(), tx));
    r = if selected { r.fill(k.c.hover) } else { r.hover(k.c.hover) };
    r
}

// ---------- the editor screen ----------

fn editor(s: &App, k: &Kit, layer: &mut Vec<El>) -> El {
    let Some(p) = s.summary() else {
        let msg = if !s.loaded {
            "Loading posts…".to_string()
        } else {
            hint(s, "Select a post or create one", "newPost").replace(" (", " with ").replace(')', "")
        };
        return col().w_full().center().child(text(msg).font(k.ty.ui).color(k.c.text_muted));
    };
    let on = p.local && !s.busy;
    let mut main = col().w_full().min_h(0.0).child(fields(s, k, on));
    if p.local {
        main = main.child(images(s, k));
    }
    main.child(body(s, k, on, layer)).child(footer(s, k, p, layer))
}

/// A dot beside a field's label when the field differs from the compared revision.
fn changed(s: &App, pick: fn(&Frontmatter) -> String) -> bool {
    s.since_post.as_ref().is_some_and(|old| pick(&old.fm) != pick(&s.fm()))
}

fn fields(s: &App, k: &Kit, on: bool) -> El {
    let name = |t: &str, dot: bool| {
        let mut r = row().w(92.0).h(30.0).gap(6.0).center_y().child(text(t.to_string()).font(k.ty.small).color(if dot { k.c.warning.fg } else { k.c.text_muted }));
        if dot {
            r = r.child(el().size(6.0, 6.0).radius(3.0).fill(k.c.warning.fg));
        }
        r
    };
    let line = |l: El, f: El| row().w_full().gap(8.0).child(l).child(f.grow(1.0));
    let watch = |e: El| e.on_input_change(app::edited);

    let mut tags = row().grow(1.0).min_h(30.0).px(6.0).py(3.0).gap(5.0).center_y().flex_wrap()
        .radius(6.0).fill(k.c.bg_base).border(1.0, k.c.border_base);
    for (i, t) in s.doc.tags.iter().enumerate() {
        let mut chip = row().h(22.0).px(7.0).gap(5.0).center_y().radius(11.0).fill(k.c.layer_02)
            .child(text(t.clone()).font(k.ty.small).color(k.c.text_base));
        if on {
            chip = chip.child(text("×").font(k.ty.small).color(k.c.text_muted).cursor(Cursor::Pointer).key(format!("tag-x-{i}"))
                .on_press_tx(move |s: &mut App, tx| app::remove_tag(s, i, tx)));
        }
        tags = tags.child(chip);
    }
    tags = tags.child(text_input(&s.doc.tag, k.ty.ui).key("tag").single_line().grow(1.0).min_w(80.0)
        .color(k.c.text_base).placeholder(if s.doc.tags.is_empty() { "Add a tag" } else { "" }, k.c.text_faint)
        .read_only(!on)
        .on_edit(|s: &mut App| &mut s.doc.tag)
        .on_submit(app::commit_tag)
        .on_input_key(|s: &mut App, ev, tx| match &ev.key {
            Key::Text(t) if t == "," => { app::commit_tag(s, tx); true }
            Key::Backspace if s.doc.tag.text.is_empty() && !s.doc.tags.is_empty() => {
                let last = s.doc.tags.len() - 1;
                app::remove_tag(s, last, tx);
                true
            }
            _ => false,
        }));

    let date = watch(field(k, "date", &s.doc.date, "YYYY-MM-DD", on, |s| &mut s.doc.date)).w(130.0);
    let draft = Checkbox::new(s.doc.draft).label("draft (hidden on the live site)").disabled(!on)
        .on_toggle_tx(|s: &mut App, tx| { s.doc.draft = !s.doc.draft; app::edited(s, tx) }).build(k);

    col().w_full().px(20.0).py(12.0).gap(6.0).border_sides([0.0, 0.0, 1.0, 0.0], k.c.border_muted)
        .child(line(name("title", changed(s, |f| f.title.clone())), watch(field(k, "title", &s.doc.title, "", on, |s| &mut s.doc.title))))
        .child(line(name("description", changed(s, |f| f.description.clone())),
            watch(field(k, "description", &s.doc.description, "One line for the blog index and the PR body", on, |s| &mut s.doc.description))))
        .child(line(name("tags", changed(s, |f| f.tags.join(","))), tags))
        .child(row().w_full().gap(8.0)
            .child(name("pubDate", changed(s, |f| format!("{} {}", f.pub_date, f.draft))))
            .child(date)
            .child(el().h(30.0).center_y().child(draft)))
}

/// The post folder's images. Clicking one inserts its markdown reference at the caret.
fn images(s: &App, k: &Kit) -> El {
    let mut strip = row().w_full().px(20.0).py(8.0).gap(8.0).center_y().border_sides([0.0, 0.0, 1.0, 0.0], k.c.border_muted);
    for name in &s.doc.files.images {
        let mut thumb = el().size(28.0, 28.0).radius(4.0).fill(k.c.layer_02);
        if let Some(img) = app::image(s, name).filter(|i| i.is_ready()) {
            thumb = thumb.pixels_fit(img.pixels(), rune::SurfaceFit::Fill).clip_rounded(4.0);
        }
        let n = name.clone();
        strip = strip.child(row().key(format!("img-{name}")).h(36.0).px(4.0).gap(6.0).center_y().radius(6.0).hover(k.c.hover).cursor(Cursor::Pointer)
            .help(format!("Insert ./{name}"))
            .child(thumb)
            .child(label(k, name.clone()))
            .on_press_tx(move |s: &mut App, tx| app::insert_image_ref(s, &n, tx)));
    }
    strip.child(label(k, "Drop images on the editor to copy them beside index.md"))
}

/// Rune spans for the markdown runs. A hidden mark keeps its bytes in the text but is drawn
/// transparent at a near-zero size, so it takes no room until the caret reaches its line.
pub fn spans(runs: &[md::Run], k: &Kit, size: f32) -> Vec<Span> {
    runs.iter().map(|r| {
        let st = r.style;
        let mut sp = Span::new(r.start, r.end);
        if st.hidden {
            sp.size = Some(0.01);
            sp.color = Some(Color::TRANSPARENT);
            return sp;
        }
        if st.heading > 0 {
            sp.size = Some(size * [1.0, 1.6, 1.35, 1.18, 1.08, 1.0, 1.0][st.heading.min(6) as usize]);
            sp.weight = Some(700.0);
        }
        if st.bold {
            sp.weight = Some(700.0);
        }
        sp.italic = st.italic;
        if st.code {
            sp.mono = true;
            sp.bg = Some(k.c.layer_02);
            sp.bg_pad = 2.0;
        }
        if st.link {
            sp.color = Some(k.c.text_accent);
            sp.underline = Some(k.c.text_accent.alpha(0.4));
        }
        if st.strike {
            sp.color = Some(k.c.text_faint);
        }
        if st.mark {
            sp.color = Some(k.c.text_faint);
            sp.weight = None;
        }
        sp
    }).collect()
}

pub fn body_style(s: &App, k: &Kit) -> TextStyle {
    let size = s.settings.editor_font_size.clamp(10, 32) as f32;
    TextStyle { size, line_height: (size * 1.65).round(), ..k.ty.body }
}

fn body(s: &App, k: &Kit, on: bool, layer: &mut Vec<El>) -> El {
    let style = body_style(s, k);
    let t = &s.doc.body.text;
    let caret = s.doc.body.focused.then(|| (s.doc.body.anchor.min(s.doc.body.focus), s.doc.body.anchor.max(s.doc.body.focus)));
    let sp = spans(&md::runs(t, caret, s.settings.live_preview), k, style.size);

    let input = text_input(&s.doc.body, style).key("body").grow(1.0).wrap()
        .color(k.c.text_base)
        .placeholder("Start writing…", k.c.text_faint)
        .spans(sp.clone())
        .read_only(!on)
        .on_edit(|s: &mut App| &mut s.doc.body)
        .on_input_change(app::edited)
        .on_input_key(|s: &mut App, ev, tx| {
            let done = app::newline(&mut s.doc.body, ev);
            if done {
                app::edited(s, tx);
            }
            done
        })
        .on_input_paste(|s: &mut App, c, tx| {
            if let Some(img) = c.image() {
                app::paste_image(s, img.png(), tx);
                true
            } else if !c.files().is_empty() {
                app::add_images(s, c.files(), tx);
                true
            } else {
                false
            }
        })
        .on_layout(|s: &mut App, r| s.editor_w = r[2]);

    let mut line = row().w_full().px(20.0).py(16.0).gap(10.0).items_start();
    if s.settings.line_numbers || !s.chunks.is_empty() {
        line = line.child(gutter(s, k, &sp, style));
    }
    line = line.child(input);
    if let Some(i) = s.marker.filter(|i| *i < s.chunks.len()) {
        layer.push(marker_popup(s, k, i));
    }
    col().w_full().grow(1.0).min_h(0.0).key("editor").scroll(ScrollSpec { stick_bottom: false })
        .accepts_extensions(IMAGE_EXTS)
        .on_drop_files(|s: &mut App, paths, tx| app::add_images(s, paths, tx))
        .child(line)
        .child(previews(s, k))
}

/// The pictures of the lines that hold only an image. A text field can't hold a widget, so
/// they sit under the text instead of under their own line, each with its line number.
fn previews(s: &App, k: &Kit) -> El {
    let mut c = col().w_full().px(20.0).pb(20.0).gap(12.0);
    if !s.settings.live_preview {
        return c;
    }
    for l in md::image_lines(&s.doc.body.text) {
        let Some([w, h]) = app::image(s, &l.url).and_then(|i| i.size()) else { continue };
        let Some(img) = app::image(s, &l.url) else { continue };
        let scale = (480.0 / w as f32).min(280.0 / h as f32).min(1.0);
        c = c.child(col().gap(4.0)
            .child(el().size(w as f32 * scale, h as f32 * scale).radius(6.0).clip_rounded(6.0).pixels_fit(img.pixels(), rune::SurfaceFit::Fit))
            .child(label(k, format!("Line {} · {}", l.line + 1, l.url))));
    }
    c
}

/// Line numbers and change markers. Rune has no per-line geometry for a text field, so each
/// logical line is measured with the same spans and wrap width the field uses, and the gutter
/// stacks boxes of those heights beside it.
fn gutter(s: &App, k: &Kit, sp: &[Span], style: TextStyle) -> El {
    let t = &s.doc.body.text;
    let width = (s.editor_w > 0.0).then_some(s.editor_w);
    let numbers = s.settings.line_numbers;
    let count = t.split('\n').count();
    let digits = count.to_string().len().max(2) as f32;
    let gw = if numbers { digits * style.size * 0.62 + 12.0 } else { 8.0 };
    let num = TextStyle { size: (style.size - 2.0).max(9.0), line_height: style.line_height, ..k.ty.mono };
    let mut g = col().w(gw);
    let (mut off, mut si) = (0usize, 0usize);
    for (i, raw) in t.split('\n').enumerate() {
        let (ls, le) = (off, off + raw.len());
        off = le + 1;
        while si < sp.len() && (sp[si].end as usize) <= ls {
            si += 1;
        }
        let own: Vec<Span> = sp[si..].iter().take_while(|x| (x.start as usize) < le).map(|x| {
            let mut y = x.clone();
            y.start = (x.start as usize).max(ls) as u32 - ls as u32;
            y.end = (x.end as usize).min(le) as u32 - ls as u32;
            y
        }).collect();
        let h = rune::text::measure(if raw.is_empty() { " " } else { raw }, &own, style, width).1.max(style.line_height);
        let mut r = row().w_full().h(h).items_start();
        r = r.child(if numbers { text((i + 1).to_string()).font(num).color(k.c.text_faint).grow(1.0).align(rune::Align::End) } else { el().grow(1.0) });
        // A chunk marks its new lines; a pure deletion marks the line it was removed before.
        let hit = s.chunks.iter().enumerate().find(|(_, c)| {
            if c.b.is_empty() { c.b.start.min(count - 1) == i } else { c.b.contains(&i) }
        });
        r = r.child(match hit {
            Some((ci, c)) => {
                let color = if c.b.is_empty() { k.c.danger.fg } else if c.a.is_empty() { k.c.success.fg } else { k.c.info.fg };
                let first = c.b.is_empty() || c.b.start == i;
                let m = el().key(if first { format!("mark-{ci}") } else { format!("mark-{ci}-{i}") })
                    .w(4.0).ml(6.0).radius(if c.b.is_empty() { 2.0 } else { 0.0 }).fill(color).cursor(Cursor::Pointer)
                    .help("Show what changed")
                    .on_press(move |s: &mut App| s.marker = if s.marker == Some(ci) { None } else { Some(ci) });
                if c.b.is_empty() { m.h(6.0) } else { m.h(h) }
            }
            None => el().w(4.0).ml(6.0),
        });
        g = g.child(r);
    }
    g
}

fn marker_popup(s: &App, k: &Kit, i: usize) -> El {
    let c = &s.chunks[i];
    let old: Vec<&str> = s.since_post.as_ref().map(|p| p.body.split('\n').collect()).unwrap_or_default();
    let was = old.get(c.a.clone()).map(|l| l.join("\n")).unwrap_or_default();
    let since = s.since.as_ref().map(|x| x.label.clone()).unwrap_or_default();
    let head = if c.a.is_empty() { format!("Added since {since}") } else if c.b.is_empty() { format!("Removed since {since}") } else { format!("Changed since {since}") };
    let mut content = col().w(420.0).padding(12.0).gap(10.0).child(label(k, head));
    if !c.a.is_empty() {
        content = content.child(col().w_full().max_h(220.0).scroll(ScrollSpec { stick_bottom: false }).padding(8.0).radius(6.0).fill(k.c.layer_02)
            .child(text(was).font(k.ty.mono).color(k.c.text_base).wrap()));
    }
    content = content.child(row().w_full().gap(8.0).justify_end()
        .child(btn(k, "Close", ButtonVariant::Ghost, true, "", |s, _| s.marker = None))
        .child(btn(k, if c.a.is_empty() { "Remove these lines" } else { "Revert" }, ButtonVariant::Neutral, !s.busy, "", move |s, tx| app::revert_chunk(s, i, tx))));
    popover::anchored(k, popover::raised(k, content, 8.0).modal(true, |s: &mut App| s.marker = None), &format!("mark-{i}"), Placement::BottomStart, 4.0)
}

/// The choices of the Changes menu: its label and the revision it compares against.
pub fn since_options(s: &App) -> Vec<(String, Option<Since>)> {
    let branch = s.project().map(|p| p.branch.clone()).unwrap_or_default();
    let mut v = vec![
        ("Changes: off".to_string(), None),
        ("Since last sync".to_string(), Some(Since { rev: "base".into(), label: "last sync".into() })),
    ];
    if s.summary().is_some_and(|p| p.on_main) {
        v.push((format!("Since {branch}"), Some(Since { rev: "main".into(), label: branch })));
    }
    for c in &s.history {
        let short: String = c.sha.chars().take(7).collect();
        v.push((format!("{short} · {} · {}", app::date_of(c.date), c.subject), Some(Since { rev: c.sha.clone(), label: short })));
    }
    // A commit picked before the history was dropped still shows as the current choice.
    if let Some(cur) = &s.since && !v.iter().any(|(_, x)| x.as_ref().is_some_and(|x| x.rev == cur.rev)) {
        v.push((cur.label.clone(), Some(cur.clone())));
    }
    v
}

fn footer(s: &App, k: &Kit, p: &PostSummary, layer: &mut Vec<El>) -> El {
    let (status, t, _) = app::status_of(p, s.doc.draft);
    let project = s.project();
    let path = if p.local {
        format!("{}/{}/index.md", project.map(|x| x.posts_dir.as_str()).unwrap_or(""), p.slug)
    } else {
        format!("On {} · read only", project.map(|x| x.branch.as_str()).unwrap_or(""))
    };
    let mut f = row().w_full().h(44.0).px(14.0).gap(10.0).center_y().border_sides([1.0, 0.0, 0.0, 0.0], k.c.border_muted)
        .child(text(status).font(k.ty.small_medium).color(tone(k, t)))
        .child(text(path).font(k.ty.small).color(k.c.text_muted).truncate().shrink());
    if p.local {
        let (l, c) = match s.doc.save {
            SaveState::Saved => ("Saved", k.c.text_muted),
            SaveState::Saving => ("Saving…", k.c.text_muted),
            SaveState::Failed => ("Save failed", k.c.danger.fg),
        };
        f = f.child(text(l).font(k.ty.small).color(c));
    }
    f = f.child(spacer());
    let free = !s.busy;
    if !p.local {
        let slug = p.slug.clone();
        return f.child(btn(k, "Edit", ButtonVariant::Contrast, free, "Start a draft of this post", move |s, _| app::edit(s, slug.clone())));
    }
    if p.conflict {
        return f;
    }
    let options = since_options(s);
    let labels: Vec<String> = options.iter().map(|o| o.0.clone()).collect();
    let current = options.iter().find(|o| o.1 == s.since).map(|o| o.0.clone());
    f = f.child(Select::new("since", &labels).current(current.as_deref()).open(s.since_open).width(190.0)
        .on_toggle(|s: &mut App| { s.since_open = !s.since_open; if s.since_open { app::load_history(s) } })
        .on_close(|s: &mut App| s.since_open = false)
        .on_choose(|s: &mut App, v: &str, _| {
            let pick = since_options(s).into_iter().find(|o| o.0 == v).and_then(|o| o.1);
            s.since_open = false;
            app::set_since(s, pick);
        })
        .build(k, layer));
    let sync = p.sync.as_ref();
    let behind = sync.map_or(0, |x| x.behind);
    let pull_first = behind > 0 || sync.is_some_and(|x| !x.base_known);
    let (can_push, why) = s.push_state();
    let run = |id: &'static str| move |s: &mut App, tx: &mut Tx| app::run_action(s, id, tx);
    if sync.is_some_and(|x| x.changes > 0) && !pull_first {
        f = f.child(btn(k, "Review", ButtonVariant::Outline, free, &hint(s, "Compare the draft with the last synced version", "review"), run("review")));
    }
    let pull = if behind > 0 { format!("↓ Pull {behind}") } else { "↓ Pull".to_string() };
    f = f.child(btn(k, &pull, if pull_first { ButtonVariant::Contrast } else { ButtonVariant::Outline }, free,
        &hint(s, "Fetch and merge remote changes into the draft", "pull"), run("pull")));
    f = f.child(btn(k, "↑ Push", if can_push { ButtonVariant::Contrast } else { ButtonVariant::Outline }, free && can_push, &hint(s, &why, "push"), run("push")));
    if s.can_open_pr() {
        f = f.child(btn(k, "Open PR", if can_push { ButtonVariant::Outline } else { ButtonVariant::Contrast }, free, &hint(s, "Open a pull request on GitHub", "openPr"), run("openPr")));
    }
    f
}

// ---------- command palette ----------

fn palette(s: &App, k: &Kit, layer: &mut Vec<El>) {
    if !s.palette.open {
        return;
    }
    let cmds = app::filtered(s);
    let index = s.palette.index.min(cmds.len().saturating_sub(1));
    let input = text_input(&s.palette.query, TextStyle { size: 15.0, ..k.ty.ui }).key("palette-input").single_line().grow(1.0).autofocus()
        .color(k.c.text_base)
        .placeholder(if s.palette.projects { "Switch to project" } else { "Type a command or a post title" }, k.c.text_faint)
        .on_edit(|s: &mut App| &mut s.palette.query)
        .on_input_change(|s: &mut App, _| s.palette.index = 0)
        .on_input_key(|s: &mut App, ev, tx| {
            let n = app::filtered(s).len();
            match ev.key {
                Key::Down => { s.palette.index = if n == 0 { 0 } else { (s.palette.index + 1) % n }; tx.scroll_into_view(&format!("cmd-{}", s.palette.index)); true }
                Key::Up => { s.palette.index = if n == 0 { 0 } else { (s.palette.index + n - 1) % n }; tx.scroll_into_view(&format!("cmd-{}", s.palette.index)); true }
                Key::Enter => {
                    if let Some(c) = app::filtered(s).into_iter().nth(s.palette.index.min(n.saturating_sub(1))) {
                        app::run_cmd(s, c.run, tx);
                    }
                    true
                }
                _ => false,
            }
        });
    let mut list = col().w_full().padding(6.0).gap(1.0);
    let mut group = "";
    for (i, c) in cmds.iter().enumerate() {
        if c.group != group {
            group = c.group;
            list = list.child(el().px(10.0).pt(8.0).pb(4.0).child(label(k, group)));
        }
        let run = c.run.clone();
        let mut r = row().key(format!("cmd-{i}")).w_full().h(32.0).px(10.0).gap(10.0).center_y().radius(6.0).cursor(Cursor::Pointer)
            .child(text(c.label.clone()).font(k.ty.ui).color(k.c.text_base).truncate().grow(1.0))
            .on_press_tx(move |s: &mut App, tx| app::run_cmd(s, run.clone(), tx));
        if !c.shortcut.is_empty() {
            r = r.child(label(k, c.shortcut.clone()));
        }
        list = list.child(if i == index { r.fill(k.c.hover) } else { r.hover(k.c.hover) });
    }
    if cmds.is_empty() {
        list = list.child(el().padding(12.0).child(label(k, "No matches")));
    }
    let panel = col().key("palette").w(560.0).max_h(440.0).radius(10.0).fill(k.c.layer_01).border(1.0, k.c.border_base)
        .child(row().w_full().h(46.0).px(14.0).center_y().child(input))
        .child(el().w_full().h(1.0).fill(k.c.border_muted))
        .child(col().w_full().min_h(0.0).shrink().scroll(ScrollSpec { stick_bottom: false }).key("palette-list").child(list))
        .modal(true, app::close_palette);
    layer.push(el().key("palette-scrim").absolute(0.0, 0.0).w_full().h_full().fill(k.c.scrim));
    layer.push(col().key("palette-layer").absolute(0.0, 0.0).w_full().h_full().pt(90.0).items_center().child(panel));
}

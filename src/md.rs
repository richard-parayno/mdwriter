//! Markdown styling for the editor: which bytes are headings, emphasis, code, links and syntax
//! marks. The text itself never changes; the editor paints these runs over one plain text field.
//! With live preview on, syntax marks away from the caret's line are collapsed (drawn at a
//! near-zero size), so `**bold**` reads as bold text until the caret enters its line.

/// How one run of bytes is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    /// 1 to 6 on a heading line.
    pub heading: u8,
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
    pub link: bool,
    /// A syntax mark: `#`, `**`, backticks, a link's brackets and URL.
    pub mark: bool,
    /// A mark to collapse: live preview is on and the caret is on another line.
    pub hidden: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    pub start: usize,
    pub end: usize,
    pub style: Style,
}

/// An image on a line of its own: `![alt](url)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageLine {
    pub line: usize,
    pub url: String,
}

/// The styled runs of `text`, in order and without overlap; unstyled text has no run.
/// `caret` is the byte range of the selection: marks on the lines it touches stay visible.
pub fn runs(text: &str, caret: Option<(usize, usize)>, live: bool) -> Vec<Run> {
    let mut styles = vec![Style::default(); text.len()];
    let mut fence = false;
    let mut at = 0;
    for line in text.split('\n') {
        let (start, end) = (at, at + line.len());
        at = end + 1;
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fence = !fence;
            paint(&mut styles, start, end, |s| { s.code = true; s.mark = true; });
            continue;
        }
        if fence {
            paint(&mut styles, start, end, |s| s.code = true);
            continue;
        }
        let hide = live && !caret.is_some_and(|(a, b)| a.min(b) <= end && a.max(b) >= start);
        let mut body = start;
        let level = line.bytes().take_while(|b| *b == b'#').count();
        if (1..=6).contains(&level) && line[level..].starts_with(' ') {
            paint(&mut styles, start, end, |s| { s.heading = level as u8; s.bold = true; });
            paint(&mut styles, start, start + level + 1, |s| { s.mark = true; s.hidden = hide; });
            body = start + level + 1;
        }
        inline(text, &mut styles, body, end, hide);
    }
    let mut out: Vec<Run> = Vec::new();
    for (i, style) in styles.iter().enumerate() {
        if *style == Style::default() { continue; }
        match out.last_mut() {
            Some(last) if last.end == i && last.style == *style => last.end = i + 1,
            _ => out.push(Run { start: i, end: i + 1, style: *style }),
        }
    }
    out
}

fn paint(styles: &mut [Style], start: usize, end: usize, f: impl Fn(&mut Style)) {
    styles[start..end].iter_mut().for_each(f);
}

fn mark(styles: &mut [Style], start: usize, end: usize, hide: bool) {
    paint(styles, start, end, |s| { s.mark = true; s.hidden = hide; });
}

/// Styles emphasis, code and links between `start` and `end`, which lie on one line.
fn inline(text: &str, styles: &mut [Style], start: usize, end: usize, hide: bool) {
    let bytes = text.as_bytes();
    let find = |from: usize, pat: &str| text.get(from..end).and_then(|s| s.find(pat)).map(|i| from + i);
    let mut i = start;
    while i < end {
        let rest = &text[i..end];
        if bytes[i] == b'\\' {
            i += 2;
        } else if bytes[i] == b'`' {
            let Some(close) = find(i + 1, "`") else { break };
            paint(styles, i, close + 1, |s| s.code = true);
            mark(styles, i, i + 1, hide);
            mark(styles, close, close + 1, hide);
            i = close + 1;
        } else if rest.starts_with("![") || bytes[i] == b'[' {
            let open = i + usize::from(bytes[i] == b'!');
            let link = find(open + 1, "](").and_then(|mid| find(mid + 2, ")").map(|close| (mid, close)));
            let Some((mid, close)) = link else { i = open + 1; continue };
            paint(styles, open + 1, mid, |s| s.link = true);
            mark(styles, i, open + 1, hide);
            mark(styles, mid, close + 1, hide);
            inline(text, styles, open + 1, mid, hide);
            i = close + 1;
        } else if let Some(delim) = ["**", "__", "~~", "*", "_"].into_iter().find(|d| rest.starts_with(d)) {
            let n = delim.len();
            // A mark opens only against text, and `_` only at a word's start (snake_case stays plain).
            let opens = bytes.get(i + n).is_some_and(|b| !b.is_ascii_whitespace())
                && (!delim.starts_with('_') || i == start || !bytes[i - 1].is_ascii_alphanumeric());
            let close = if opens { find(i + n, delim).filter(|c| *c > i + n) } else { None };
            let Some(close) = close else { i += n; continue };
            paint(styles, i, close + n, |s| match delim {
                "~~" => s.strike = true,
                "**" | "__" => s.bold = true,
                _ => s.italic = true,
            });
            mark(styles, i, i + n, hide);
            mark(styles, close, close + n, hide);
            inline(text, styles, i + n, close, hide);
            i = close + n;
        } else {
            i += rest.chars().next().map_or(1, char::len_utf8);
        }
    }
}

/// The lines that hold only an image, for the preview shown beside the text.
pub fn image_lines(text: &str) -> Vec<ImageLine> {
    let mut fence = false;
    text.split('\n').enumerate().filter_map(|(line, raw)| {
        let t = raw.trim();
        if t.starts_with("```") || t.starts_with("~~~") { fence = !fence; }
        if fence { return None; }
        let inner = t.strip_prefix("![")?.strip_suffix(')')?;
        let (_, url) = inner.split_once("](")?;
        (!url.contains(')') && !url.is_empty()).then(|| ImageLine { line, url: url.split_whitespace().next().unwrap_or(url).to_string() })
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(runs: &[Run], text: &str, needle: &str) -> Style {
        let i = text.find(needle).unwrap();
        runs.iter().find(|r| r.start <= i && i < r.end).map(|r| r.style).unwrap_or_default()
    }

    #[test]
    fn marks_hide_away_from_the_caret_line_only_in_live_preview() {
        let text = "# Title\n\nSome **bold** and `code` here\nand a [link](https://example.com).";
        let caret = text.find("Some").map(|i| (i, i));
        let live = runs(text, caret, true);
        assert_eq!(at(&live, text, "# "), Style { heading: 1, bold: true, mark: true, hidden: true, ..Default::default() });
        assert_eq!(at(&live, text, "Title"), Style { heading: 1, bold: true, ..Default::default() });
        assert!(at(&live, text, "**bold").mark && !at(&live, text, "**bold").hidden, "the caret's line shows its marks");
        assert!(at(&live, text, "bold").bold && !at(&live, text, "bold").mark);
        assert!(at(&live, text, "code").code);
        assert!(at(&live, text, "link").link && !at(&live, text, "link").hidden);
        assert!(at(&live, text, "](https").hidden && at(&live, text, "example").hidden);
        assert!(runs(text, caret, false).iter().all(|r| !r.style.hidden));
    }

    #[test]
    fn code_fences_and_identifiers_are_left_alone() {
        let text = "snake_case_name and 2 * 3 * 4\n```\n**not bold**\n```\n~~gone~~ _it_";
        let r = runs(text, None, true);
        assert_eq!(at(&r, text, "case"), Style::default());
        assert_eq!(at(&r, text, "3"), Style::default());
        assert_eq!(at(&r, text, "not bold"), Style { code: true, ..Default::default() });
        assert!(at(&r, text, "gone").strike);
        assert!(at(&r, text, "it").italic);
    }

    #[test]
    fn images_on_their_own_line_are_found() {
        let text = "Intro ![inline](./a.png) text\n\n![Cover photo](./cover.png)\n```\n![code](./no.png)\n```\n  ![Remote](https://x.test/y.jpg \"t\")";
        assert_eq!(image_lines(text), [
            ImageLine { line: 2, url: "./cover.png".into() },
            ImageLine { line: 6, url: "https://x.test/y.jpg".into() },
        ]);
    }
}

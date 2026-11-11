//! A post's text: YAML frontmatter between `---` fences, then the Markdown body.
//! The five known fields mirror the `posts` collection schema of the blog; any other key is
//! kept as it was and written back after them.

use serde_yaml_ng::{Mapping, Value};
use unicode_normalization::UnicodeNormalization;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Frontmatter {
    pub title: String,
    pub description: String,
    /// YYYY-MM-DD
    pub pub_date: String,
    pub tags: Vec<String>,
    pub draft: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Post {
    pub fm: Frontmatter,
    pub body: String,
    pub extra: Mapping,
}

/// Splits `raw` into the YAML between the fences and what follows them.
fn split(raw: &str) -> Option<(&str, &str)> {
    let open = if raw.starts_with("---\r\n") { 5 } else if raw.starts_with("---\n") { 4 } else { return None };
    // The opening fence's newline may also be the one before the closing fence (no keys at all).
    let close = (open - 1) + raw[open - 1..].find("\n---")?;
    let yaml = raw.get(open..close).unwrap_or("");
    let rest = &raw[close + 4..];
    let rest = rest.strip_prefix("\r\n").or_else(|| rest.strip_prefix('\n')).unwrap_or(rest);
    Some((yaml.strip_suffix('\r').unwrap_or(yaml), rest))
}

fn scalar(v: Option<Value>) -> String {
    match v {
        Some(Value::String(s)) => s,
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        _ => String::new(),
    }
}

fn is_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 10 && b[..10].iter().enumerate().all(|(i, c)| if i == 4 || i == 7 { *c == b'-' } else { c.is_ascii_digit() })
}

pub fn parse(raw: &str) -> Post {
    let (yaml, rest) = match split(raw) {
        Some(parts) => parts,
        None => return Post { fm: Frontmatter { pub_date: today(), ..Default::default() }, body: raw.to_string(), extra: Mapping::new() },
    };
    let mut data = serde_yaml_ng::from_str::<Mapping>(yaml).unwrap_or_default();
    let mut take = |key: &str| data.remove(key);
    let title = scalar(take("title"));
    let description = scalar(take("description"));
    let date = scalar(take("pubDate"));
    let tags = match take("tags") {
        Some(Value::Sequence(items)) => items.into_iter().map(|v| scalar(Some(v))).collect(),
        _ => Vec::new(),
    };
    let draft = take("draft") == Some(Value::Bool(true));
    let body = rest.strip_prefix("\r\n").or_else(|| rest.strip_prefix('\n')).unwrap_or(rest);
    Post {
        fm: Frontmatter { title, description, pub_date: if is_date(&date) { date[..10].to_string() } else { today() }, tags, draft },
        body: body.to_string(),
        extra: data,
    }
}

pub fn serialize(post: &Post) -> String {
    let q = |s: &str| serde_json::to_string(s).unwrap_or_default();
    let fm = &post.fm;
    let mut lines = vec![
        "---".to_string(),
        format!("title: {}", q(&fm.title)),
        format!("description: {}", q(&fm.description)),
        format!("pubDate: {}", fm.pub_date),
    ];
    if fm.tags.is_empty() {
        lines.push("tags: []".into());
    } else {
        lines.push("tags:".into());
        lines.extend(fm.tags.iter().map(|t| format!("  - {}", q(t))));
    }
    lines.push(format!("draft: {}", fm.draft));
    if !post.extra.is_empty() {
        lines.push(serde_yaml_ng::to_string(&post.extra).unwrap_or_default().trim_end().to_string());
    }
    lines.extend(["---".to_string(), String::new(), post.body.clone()]);
    lines.join("\n")
}

/// Today's date in the local time zone.
pub fn today() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let (y, m, d) = civil(secs + local_offset(secs));
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(unix)]
fn local_offset(secs: i64) -> i64 {
    // SAFETY: `localtime_r` only writes the `tm` it is given.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        let t = secs as libc::time_t;
        if libc::localtime_r(&t, &mut tm).is_null() { 0 } else { tm.tm_gmtoff as i64 }
    }
}

#[cfg(not(unix))]
fn local_offset(_: i64) -> i64 { 0 }

/// The calendar date of a count of seconds since 1970 (Howard Hinnant's `civil_from_days`).
pub fn civil(secs: i64) -> (i64, i64, i64) {
    let z = secs.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// A title as a folder name: lowercase ASCII letters and digits joined by single dashes, at most 60 long.
pub fn slugify(title: &str) -> String {
    let mut out = String::new();
    for c in title.to_lowercase().nfkd().filter(|c| !('\u{300}'..='\u{36f}').contains(c)) {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.truncate(60);
    out.trim_end_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_post_round_trips_and_keeps_unknown_keys_after_the_known_ones() {
        let raw = "---\ntitle: \"Hello: world\"\ndescription: \"One \\\"line\\\"\"\npubDate: 2024-03-09\ntags:\n  - \"rust\"\n  - \"ui\"\ndraft: true\nheroImage: ./cover.png\n---\n\nFirst line\n\n---\n\nAfter a rule\n";
        let post = parse(raw);
        assert_eq!(post.fm.title, "Hello: world");
        assert_eq!(post.fm.description, "One \"line\"");
        assert_eq!(post.fm.pub_date, "2024-03-09");
        assert_eq!(post.fm.tags, ["rust", "ui"]);
        assert!(post.fm.draft);
        assert_eq!(post.body, "First line\n\n---\n\nAfter a rule\n");
        assert_eq!(post.extra.get("heroImage").and_then(Value::as_str), Some("./cover.png"));
        assert_eq!(serialize(&post), raw);
    }

    #[test]
    fn text_without_frontmatter_is_all_body_and_missing_fields_get_defaults() {
        let post = parse("# Just text\n");
        assert_eq!(post.body, "# Just text\n");
        assert_eq!(post.fm.pub_date, today());

        let post = parse("---\r\ntitle: 7\r\npubDate: soon\r\ntags: nope\r\ndraft: \"true\"\r\n---\r\nBody");
        assert_eq!(post.fm.title, "7");
        assert_eq!(post.fm.pub_date, today());
        assert!(post.fm.tags.is_empty());
        assert!(!post.fm.draft, "only a real boolean hides a post");
        assert_eq!(post.body, "Body");
        assert!(serialize(&post).contains("tags: []\ndraft: false\n---\n\nBody"));
    }

    #[test]
    fn slugs_drop_accents_and_trailing_dashes() {
        assert_eq!(slugify("  Café au lait — 2024! "), "cafe-au-lait-2024");
        assert_eq!(slugify("???"), "");
        let long = slugify(&format!("{} b", "a".repeat(59)));
        assert_eq!(long, "a".repeat(59), "a cut that ends on a dash loses it");
    }

    #[test]
    fn dates_come_from_epoch_seconds() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(951_782_400), (2000, 2, 29));
        assert_eq!(civil(1_794_355_200 + 86_399), (2026, 11, 11));
        assert_eq!(civil(-1), (1969, 12, 31));
    }
}

//! A line diff for the change markers and the conflict view: the ranges of lines that differ
//! between two texts, from a longest-common-subsequence table over the lines that remain after
//! the shared start and end are set aside.

use std::ops::Range;

/// Lines `a` of the old text were replaced by lines `b` of the new one. Either may be empty.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chunk {
    pub a: Range<usize>,
    pub b: Range<usize>,
}

/// Above this many table cells the middle is reported as one changed chunk.
const MAX_CELLS: usize = 4_000_000;

pub fn chunks(old: &[&str], new: &[&str]) -> Vec<Chunk> {
    let head = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let tail = old[head..].iter().rev().zip(new[head..].iter().rev()).take_while(|(a, b)| a == b).count();
    let (a, b) = (&old[head..old.len() - tail], &new[head..new.len() - tail]);
    if a.is_empty() && b.is_empty() { return Vec::new(); }
    if a.is_empty() || b.is_empty() || a.len().saturating_mul(b.len()) > MAX_CELLS {
        return vec![Chunk { a: head..head + a.len(), b: head..head + b.len() }];
    }
    // lcs[i][j]: the longest common subsequence of a[i..] and b[j..].
    let w = b.len() + 1;
    let mut lcs = vec![0u32; (a.len() + 1) * w];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            lcs[i * w + j] = if a[i] == b[j] { lcs[(i + 1) * w + j + 1] + 1 } else { lcs[(i + 1) * w + j].max(lcs[i * w + j + 1]) };
        }
    }
    let mut out: Vec<Chunk> = Vec::new();
    let (mut i, mut j) = (0, 0);
    let mut open: Option<(usize, usize)> = None;
    while i < a.len() || j < b.len() {
        if i < a.len() && j < b.len() && a[i] == b[j] {
            if let Some((si, sj)) = open.take() { out.push(Chunk { a: head + si..head + i, b: head + sj..head + j }); }
            i += 1;
            j += 1;
            continue;
        }
        open.get_or_insert((i, j));
        if j == b.len() || (i < a.len() && lcs[(i + 1) * w + j] >= lcs[i * w + j + 1]) { i += 1 } else { j += 1 }
    }
    if let Some((si, sj)) = open { out.push(Chunk { a: head + si..head + i, b: head + sj..head + j }); }
    out
}

/// The text with `chunk` put back to the old lines, as the marker's Revert does.
pub fn revert(old: &[&str], new: &[&str], chunk: &Chunk) -> String {
    let mut lines: Vec<&str> = new[..chunk.b.start].to_vec();
    lines.extend(&old[chunk.a.clone()]);
    lines.extend(&new[chunk.b.end..]);
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diff(old: &str, new: &str) -> Vec<Chunk> {
        chunks(&old.split('\n').collect::<Vec<_>>(), &new.split('\n').collect::<Vec<_>>())
    }

    #[test]
    fn added_changed_and_removed_lines_are_separate_chunks() {
        let old = "one\ntwo\nthree\nfour\nfive\nsix";
        let new = "one\nTWO\nthree\nfour\nnew\nfive";
        assert_eq!(diff(old, new), [Chunk { a: 1..2, b: 1..2 }, Chunk { a: 4..4, b: 4..5 }, Chunk { a: 5..6, b: 6..6 }]);
        assert_eq!(diff(old, old), []);
        assert_eq!(diff("", "a\nb"), [Chunk { a: 0..1, b: 0..2 }]);
    }

    #[test]
    fn reverting_every_chunk_from_the_end_gives_the_old_text_back() {
        let old = "a\nb\nc\nd\ne\nf\ng";
        let new = "a\nx\nc\ne\nf\ny\nz\ng\nh";
        let (o, n): (Vec<&str>, Vec<&str>) = (old.split('\n').collect(), new.split('\n').collect());
        let mut text = new.to_string();
        for _ in 0..chunks(&o, &n).len() {
            let lines: Vec<&str> = text.split('\n').collect();
            let last = chunks(&o, &lines).pop().unwrap();
            text = revert(&o, &lines, &last);
        }
        assert_eq!(text, old);
    }
}

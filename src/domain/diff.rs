//! Line-level comparison for revision history.
//!
//! A longest-common-subsequence alignment keeps unchanged lines in place so
//! a writer can see exactly what a restore would put back; very large inputs
//! fall back to a plain before/after listing rather than an expensive
//! alignment.

use serde::Serialize;

/// Beyond this many lines on either side the quadratic alignment is skipped.
const ALIGNMENT_LIMIT: usize = 2_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffKind {
    Same,
    Added,
    Removed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DiffLine {
    pub kind: DiffKind,
    pub text: String,
}

/// Compares `before` with `after` line by line. `Removed` lines exist only in
/// `before`, `Added` lines only in `after`; the sequence reads top to bottom
/// as the merged document.
#[must_use]
pub fn diff_lines(before: &str, after: &str) -> Vec<DiffLine> {
    let old: Vec<&str> = before.lines().collect();
    let new: Vec<&str> = after.lines().collect();
    if old.len() > ALIGNMENT_LIMIT || new.len() > ALIGNMENT_LIMIT {
        return old
            .iter()
            .map(|line| DiffLine {
                kind: DiffKind::Removed,
                text: (*line).to_owned(),
            })
            .chain(new.iter().map(|line| DiffLine {
                kind: DiffKind::Added,
                text: (*line).to_owned(),
            }))
            .collect();
    }

    // lengths[i][j] = LCS length of old[i..] and new[j..]
    let width = new.len() + 1;
    let mut lengths = vec![0_u32; (old.len() + 1) * width];
    for i in (0..old.len()).rev() {
        for j in (0..new.len()).rev() {
            lengths[i * width + j] = if old[i] == new[j] {
                lengths[(i + 1) * width + j + 1] + 1
            } else {
                lengths[(i + 1) * width + j].max(lengths[i * width + j + 1])
            };
        }
    }

    let mut lines = Vec::with_capacity(old.len().max(new.len()));
    let (mut i, mut j) = (0, 0);
    while i < old.len() && j < new.len() {
        if old[i] == new[j] {
            lines.push(DiffLine {
                kind: DiffKind::Same,
                text: old[i].to_owned(),
            });
            i += 1;
            j += 1;
        } else if lengths[(i + 1) * width + j] >= lengths[i * width + j + 1] {
            lines.push(DiffLine {
                kind: DiffKind::Removed,
                text: old[i].to_owned(),
            });
            i += 1;
        } else {
            lines.push(DiffLine {
                kind: DiffKind::Added,
                text: new[j].to_owned(),
            });
            j += 1;
        }
    }
    lines.extend(old[i..].iter().map(|line| DiffLine {
        kind: DiffKind::Removed,
        text: (*line).to_owned(),
    }));
    lines.extend(new[j..].iter().map(|line| DiffLine {
        kind: DiffKind::Added,
        text: (*line).to_owned(),
    }));
    lines
}

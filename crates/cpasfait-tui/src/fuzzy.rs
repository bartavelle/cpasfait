//! Small fuzzy matcher (fzf-like): every query word must match the text as a
//! subsequence; consecutive characters and word starts score higher, gaps
//! cost a little. Case-insensitive unless the query contains an uppercase letter.

/// Score of the best alignment and the matched char indices (sorted), or
/// `None` if some query word does not match.
pub fn fuzzy_match(query: &str, text: &str) -> Option<(i64, Vec<usize>)> {
    let case_sensitive = query.chars().any(char::is_uppercase);
    let fold = |c: char| {
        if case_sensitive {
            c
        } else {
            c.to_lowercase().next().unwrap_or(c)
        }
    };
    let text: Vec<char> = text.chars().collect();
    let folded: Vec<char> = text.iter().map(|&c| fold(c)).collect();
    let mut total = 0;
    let mut positions = vec![];
    for word in query.split_whitespace() {
        let pat: Vec<char> = word.chars().map(fold).collect();
        let (s, pos) = match_word(&pat, &text, &folded)?;
        total += s;
        positions.extend(pos);
    }
    positions.sort_unstable();
    positions.dedup();
    Some((total, positions))
}

const MATCH: i64 = 16;
const CONSECUTIVE: i64 = 24;
const WORD_START: i64 = 12;
const FIRST_CHAR: i64 = 8;
const GAP: i64 = 1;

fn is_word_start(text: &[char], j: usize) -> bool {
    j == 0 || {
        let (p, c) = (text[j - 1], text[j]);
        !p.is_alphanumeric() && c.is_alphanumeric() || p.is_lowercase() && c.is_uppercase()
    }
}

/// Best alignment of `pat` in `folded` (dynamic programming, O(|pat|·|text|)).
fn match_word(pat: &[char], text: &[char], folded: &[char]) -> Option<(i64, Vec<usize>)> {
    let (m, n) = (pat.len(), folded.len());
    if m == 0 {
        return Some((0, vec![]));
    }
    // Cheap rejection before the quadratic part.
    let mut it = folded.iter();
    if m > n || !pat.iter().all(|p| it.any(|c| c == p)) {
        return None;
    }
    const NONE: i64 = i64::MIN / 2;
    // best[i][j]: best score with pat[..=i] matched and pat[i] at text[j].
    let mut best = vec![vec![NONE; n]; m];
    let mut from = vec![vec![usize::MAX; n]; m];
    for i in 0..m {
        // Running best of best[i-1][k] - GAP*(j-k-1) over k < j-1, kept as
        // (value + GAP*k) to update in O(1) per j.
        let mut run: (i64, usize) = (NONE, usize::MAX);
        for j in i..n {
            if i > 0 && j >= 2 && best[i - 1][j - 2] > NONE {
                let k = j - 2;
                let v = best[i - 1][k] + GAP * k as i64;
                if v > run.0 {
                    run = (v, k);
                }
            }
            if folded[j] != pat[i] {
                continue;
            }
            let mut bonus = MATCH;
            if is_word_start(text, j) {
                bonus += WORD_START;
            }
            if i == 0 {
                if j == 0 {
                    bonus += FIRST_CHAR;
                }
                // Leading gap is cheap.
                best[0][j] = bonus - (j as i64).min(8);
                continue;
            }
            let mut cand = (NONE, usize::MAX);
            if best[i - 1][j - 1] > NONE {
                cand = (best[i - 1][j - 1] + CONSECUTIVE, j - 1);
            }
            if run.0 > NONE {
                let v = run.0 - GAP * (j as i64 - 1);
                if v > cand.0 {
                    cand = (v, run.1);
                }
            }
            if cand.0 > NONE {
                best[i][j] = cand.0 + bonus;
                from[i][j] = cand.1;
            }
        }
    }
    let (mut j, score) = best[m - 1]
        .iter()
        .enumerate()
        .filter(|(_, s)| **s > NONE)
        .max_by_key(|(j, s)| (**s, std::cmp::Reverse(*j)))
        .map(|(j, s)| (j, *s))?;
    let mut pos = vec![0; m];
    for i in (0..m).rev() {
        pos[i] = j;
        j = from[i][j];
    }
    Some((score, pos))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(q: &str, t: &str) -> Vec<usize> {
        fuzzy_match(q, t).unwrap().1
    }

    #[test]
    fn matches_subsequences() {
        assert!(fuzzy_match("xyz", "hello").is_none());
        assert!(fuzzy_match("hlo", "hello").is_some());
        assert_eq!(pos("", "abc"), Vec::<usize>::new());
        // all words must match, in any order
        assert!(fuzzy_match("wor hel", "hello world").is_some());
        assert!(fuzzy_match("wor xyz", "hello world").is_none());
    }

    #[test]
    fn prefers_good_alignments() {
        // consecutive over scattered
        assert_eq!(pos("bar", "b_a_r bar"), vec![6, 7, 8]);
        // word starts
        assert_eq!(pos("fb", "xfoo-bar"), vec![1, 5]);
        let s = |q, t| fuzzy_match(q, t).unwrap().0;
        assert!(s("rep", "write report") > s("rep", "prepare"));
        assert!(s("tax", "Taxes 2026") > s("tax", "contact a taxi"));
    }

    #[test]
    fn smart_case() {
        assert!(fuzzy_match("abc", "ABC").is_some());
        assert!(fuzzy_match("Abc", "abc").is_none());
        assert!(fuzzy_match("été", "Été").is_some());
    }
}

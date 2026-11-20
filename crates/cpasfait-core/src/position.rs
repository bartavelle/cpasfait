//! Fractional indexing: order keys for siblings such that a key can always be
//! generated strictly between two others without renumbering.
//!
//! Keys are non-empty strings over the base-62 alphabet `0-9A-Za-z` (ASCII
//! order), never ending with `0`; they compare as plain strings.

const DIGITS: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
const BASE: usize = 62;

fn digit_value(c: u8) -> usize {
    match c {
        b'0'..=b'9' => (c - b'0') as usize,
        b'A'..=b'Z' => (c - b'A') as usize + 10,
        b'a'..=b'z' => (c - b'a') as usize + 36,
        _ => 0,
    }
}

/// Return a key strictly between `a` and `b` (`None` = open bound).
/// Precondition: `a < b` when both are given. Invalid characters are treated as `0`.
pub fn key_between(a: Option<&str>, b: Option<&str>) -> String {
    let a = a.unwrap_or("").as_bytes();
    let b = b.map(str::as_bytes);
    if let Some(b) = b {
        debug_assert!(a < b, "key_between called with a >= b");
        if a >= b {
            // Defensive: keep going with an open upper bound.
            return String::from_utf8(midpoint(a, None)).unwrap();
        }
    }
    String::from_utf8(midpoint(a, b)).unwrap()
}

fn midpoint(a: &[u8], b: Option<&[u8]>) -> Vec<u8> {
    if let Some(b) = b {
        // Strip the common prefix (a is padded with '0's).
        let mut n = 0;
        while n < b.len() && a.get(n).copied().unwrap_or(b'0') == b[n] {
            n += 1;
        }
        if n > 0 {
            let mut out = b[..n].to_vec();
            let rest_a = if n < a.len() { &a[n..] } else { &[][..] };
            out.extend(midpoint(rest_a, Some(&b[n..])));
            return out;
        }
    }
    let da = a.first().map(|&c| digit_value(c)).unwrap_or(0);
    let db = b.map(|b| digit_value(b[0])).unwrap_or(BASE);
    if db - da > 1 {
        vec![DIGITS[(da + db) / 2]]
    } else if let Some(b) = b.filter(|b| b.len() > 1) {
        vec![b[0]]
    } else {
        let mut out = vec![DIGITS[da]];
        let rest_a = if a.len() > 1 { &a[1..] } else { &[][..] };
        out.extend(midpoint(rest_a, None));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn basic() {
        let m = key_between(None, None);
        let l = key_between(None, Some(&m));
        let r = key_between(Some(&m), None);
        assert!(l < m && m < r);
    }

    #[test]
    fn many_appends_and_prepends() {
        let mut k = key_between(None, None);
        for _ in 0..500 {
            let n = key_between(Some(&k), None);
            assert!(n > k);
            k = n;
        }
        let mut k = key_between(None, None);
        for _ in 0..500 {
            let n = key_between(None, Some(&k));
            assert!(n < k, "{n} !< {k}");
            assert!(!n.ends_with('0'));
            k = n;
        }
    }

    proptest! {
        #[test]
        fn random_inserts_stay_sorted(ops in proptest::collection::vec(0usize..1000, 1..200)) {
            let mut keys: Vec<String> = vec![];
            for o in ops {
                let i = if keys.is_empty() { 0 } else { o % (keys.len() + 1) };
                let a = if i == 0 { None } else { Some(keys[i - 1].as_str()) };
                let b = keys.get(i).map(String::as_str);
                let k = key_between(a, b);
                prop_assert!(!k.ends_with('0'));
                if let Some(a) = a { prop_assert!(a < k.as_str()); }
                if let Some(b) = b { prop_assert!(k.as_str() < b); }
                keys.insert(i, k);
            }
        }
    }
}

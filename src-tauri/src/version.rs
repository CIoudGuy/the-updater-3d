//! Version strings from three ecosystems ("27.11", "12.2.0.5909", "0.12.0", "5.2.2").
//! Compared numerically part by part; missing parts count as zero, so "27.11" == "27.11.0".

use std::cmp::Ordering;

pub fn parts(v: &str) -> Vec<u64> {
    let v = v.trim().trim_start_matches(['v', 'V']);
    // Stop at a pre-release or build suffix: "5.3.0-beta+v53" -> "5.3.0"
    let core = v.split(['-', '+', ' ']).next().unwrap_or("");
    core.split('.')
        .map(|p| {
            let digits: String = p.chars().take_while(|c| c.is_ascii_digit()).collect();
            digits.parse::<u64>().unwrap_or(0)
        })
        .collect()
}

pub fn cmp(a: &str, b: &str) -> Ordering {
    let (pa, pb) = (parts(a), parts(b));
    let n = pa.len().max(pb.len());
    for i in 0..n {
        let x = pa.get(i).copied().unwrap_or(0);
        let y = pb.get(i).copied().unwrap_or(0);
        match x.cmp(&y) {
            Ordering::Equal => continue,
            o => return o,
        }
    }
    Ordering::Equal
}

pub fn newer(a: &str, than: &str) -> bool {
    cmp(a, than) == Ordering::Greater
}

/// First two parts: "5.2.2" -> "5.2"
pub fn series(v: &str) -> String {
    let p = parts(v);
    format!("{}.{}", p.first().copied().unwrap_or(0), p.get(1).copied().unwrap_or(0))
}

pub fn major(v: &str) -> u64 {
    parts(v).first().copied().unwrap_or(0)
}

/// Keep only as many parts as `like` has, so a file version "12.2.0.5909" displays as "12.2.0".
pub fn trim_like(v: &str, like: &str) -> String {
    let n = like.split('.').count().max(1);
    v.split('.').take(n).collect::<Vec<_>>().join(".")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_numerically() {
        assert!(newer("27.11", "27.2"));
        assert!(newer("12.2.0", "12.1.5"));
        assert!(newer("0.12.0", "0.9.0"));
        assert_eq!(cmp("27.11", "27.11.0"), Ordering::Equal);
        assert!(newer("v0.6.1", "0.4.1"));
        assert_eq!(cmp("5.3.0-beta+v53.873f", "5.3.0"), Ordering::Equal);
        assert!(!newer("5.2.2", "5.2.2"));
    }

    #[test]
    fn series_and_trim() {
        assert_eq!(series("5.2.2"), "5.2");
        assert_eq!(series("4.5"), "4.5");
        assert_eq!(trim_like("12.2.0.5909", "12.1.5"), "12.2.0");
        assert_eq!(major("27.11"), 27);
    }
}

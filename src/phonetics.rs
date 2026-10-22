//! Jiandao phonetic rules, independently applied to external pinyin readings.
//! Mapping facts: keytao docs phonetics-rules.html, alt-code.html and layouts.html.
use std::collections::BTreeSet;
use unicode_normalization::UnicodeNormalization;

pub fn normalize_pinyin(raw: &str) -> String {
    let mut result = String::new();
    for c in raw.trim().to_lowercase().replace("u:", "v").nfd() {
        match c {
            '\u{308}' if result.ends_with('u') => {
                result.pop();
                result.push('v');
            }
            '\u{300}' | '\u{301}' | '\u{302}' | '\u{304}' | '\u{30c}' => {}
            _ => result.push(c),
        }
    }
    if result.ends_with(['1', '2', '3', '4', '5', '0']) {
        result.pop();
    }
    result
}

/// No guessed key for an unknown/abbreviated/syllabic reading. All legal fly keys
/// are accepted; tone distinctions cannot be checked by a toneless input scheme.
pub fn phonetic_codes(pinyin: &str) -> BTreeSet<String> {
    let p = normalize_pinyin(pinyin);
    if p == "ng" {
        return BTreeSet::from(["xr".into()]);
    }
    let initial = [
        "zh", "ch", "sh", "b", "p", "m", "f", "d", "t", "n", "l", "g", "k", "h", "j", "q", "x",
        "r", "z", "c", "s", "y", "w",
    ]
    .into_iter()
    .find(|i| p.starts_with(i))
    .unwrap_or("");
    let final_part = &p[initial.len()..];
    if initial.is_empty() && !p.starts_with(['a', 'e', 'o']) {
        return BTreeSet::new();
    }
    let final_keys = match final_part {
        "ua" | "iu" => "q",
        "ei" | "un" | "vn" => "w",
        "e" => "e",
        "eng" | "ueng" | "ng" => "r",
        "uan" | "van" => "t",
        "ong" | "iong" => "y",
        "ang" => "p",
        "a" | "ia" => "s",
        "ou" | "ie" => "d",
        "an" => "f",
        "uai" | "ing" => "g",
        "ai" | "ue" | "ve" => "h",
        "u" if ["j", "q", "x", "y"].contains(&initial) => "l",
        "u" | "er" => "j",
        "i" => "k",
        "uo" | "v" | "o" => "l",
        "in" | "ui" => "b",
        "en" => "n",
        "ian" => "m",
        "uang" => "mx",
        "iang" => "x",
        "iao" => "c",
        "ao" => "z",
        _ => return BTreeSet::new(),
    };
    let initial_keys = match initial {
        "zh" if ["ai", "ao", "e"].contains(&final_part) => "qf",
        "zh" if ["an", "ang", "ei", "en", "eng", "u", "un"].contains(&final_part) => "q",
        "zh" => "f",
        "ch" if ["ao", "e"].contains(&final_part) => "jw",
        "ch" if ["ai", "an", "ang", "en", "eng", "u", "un"].contains(&final_part) => "j",
        "ch" => "w",
        "sh" => "e",
        "" => "x",
        other => other,
    };
    initial_keys
        .chars()
        .flat_map(|i| final_keys.chars().map(move |f| format!("{i}{f}")))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalization_preserves_umlaut_and_rejects_uninterpreted_readings() {
        assert_eq!(normalize_pinyin("nǚ"), "nv");
        assert_eq!(normalize_pinyin("Nu:3"), "nv");
        assert_eq!(normalize_pinyin("ê̄"), "e");
        assert!(phonetic_codes("m̄").is_empty());
        assert!(phonetic_codes("xx5").is_empty());
        assert!(phonetic_codes("lu:4").contains("ll"));
    }
    #[test]
    fn scheme_special_cases_and_fly_keys() {
        for (p, expected) in [
            ("zhi4", vec!["fk"]),
            ("zhu4", vec!["qj"]),
            ("zhai3", vec!["qh", "fh"]),
            ("che1", vec!["je", "we"]),
            ("chuang2", vec!["wm", "wx"]),
            ("shuang1", vec!["em", "ex"]),
            ("ju4", vec!["jl"]),
            ("qun2", vec!["qw"]),
            ("yuan2", vec!["yt"]),
            ("yue4", vec!["yh"]),
            ("an1", vec!["xf"]),
            ("er2", vec!["xj"]),
            ("ng2", vec!["xr"]),
            ("hng5", vec!["hr"]),
        ] {
            assert_eq!(
                phonetic_codes(p),
                expected.into_iter().map(str::to_owned).collect(),
                "{p}"
            );
        }
    }
}

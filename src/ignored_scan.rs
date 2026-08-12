//! Matching of destination paths against the rendered `.chezmoiignore` rules.
//!
//! `chezmoi unmanaged` applies `.chezmoiignore` itself, so the top level of the
//! Unmanaged view is ignore-correct. Expanding a directory in that view reads
//! children straight from the filesystem, which would otherwise surface files
//! chezmoi deliberately hides (including secrets that are ignored *because*
//! they are secret). This module provides the matcher used to re-apply the
//! ignore rules during that descent.

use regex::Regex;

/// Sentinel path component used to probe whether an entire directory subtree is
/// ignored (see [`IgnoreMatcher::dir_fully_ignored`]).
const PROBE: &str = "__chezmoi_probe__";

/// An ordered set of `.chezmoiignore` rules. Matching follows chezmoi/gitignore
/// "last match wins" semantics: a later `!`-negated rule can re-include a path
/// excluded by an earlier rule.
pub struct IgnoreMatcher {
    rules: Vec<Rule>,
}

struct Rule {
    negated: bool,
    regex: Regex,
}

impl IgnoreMatcher {
    /// Build a matcher from already-rendered `.chezmoiignore` lines. Comments
    /// (`#`) and blank lines are skipped; a leading `!` marks a negation.
    pub fn from_patterns<I, S>(lines: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut rules = Vec::new();
        for line in lines {
            let raw = line.as_ref().trim();
            if raw.is_empty() || raw.starts_with('#') {
                continue;
            }
            let (negated, pattern) = match raw.strip_prefix('!') {
                Some(rest) => (true, rest),
                None => (false, raw),
            };
            if pattern.is_empty() {
                continue;
            }
            if let Ok(regex) = Regex::new(&translate_pattern(pattern)) {
                rules.push(Rule { negated, regex });
            }
        }
        Self { rules }
    }

    /// Whether `rel` (a `/`-separated path relative to the destination) is
    /// ignored. Empty when no rule matches.
    pub fn is_ignored(&self, rel: &str) -> bool {
        let mut ignored = false;
        for rule in &self.rules {
            if rule.regex.is_match(rel) {
                ignored = !rule.negated;
            }
        }
        ignored
    }

    /// Whether every path under directory `rel` is ignored. Probes with a
    /// synthetic deep child: a `Foo/**` rule matches it, so `Foo` collapses.
    ///
    /// Caveat: a directory with an active re-include exception
    /// (`Foo/**` + `!Foo/keep`) can be wrongly collapsed here. The current
    /// `.chezmoiignore` has no such combination.
    pub fn dir_fully_ignored(&self, rel: &str) -> bool {
        self.is_ignored(&format!("{rel}/{PROBE}/{PROBE}"))
    }
}

/// Translate a `doublestar` glob into an anchored regex.
///
/// `**` matches across `/`; `*` and `?` stop at `/`. `[...]` classes are treated
/// literally (chezmoi supports them, but none appear in typical ignore files and
/// treating them literally is the safe choice).
fn translate_pattern(pattern: &str) -> String {
    let mut re = String::from("^");
    let mut literal = String::new();
    let mut chars = pattern.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '*' => {
                flush_literal(&mut re, &mut literal);
                if chars.peek() == Some(&'*') {
                    chars.next();
                    re.push_str(".*");
                } else {
                    re.push_str("[^/]*");
                }
            }
            '?' => {
                flush_literal(&mut re, &mut literal);
                re.push_str("[^/]");
            }
            '\\' => {
                if let Some(next) = chars.next() {
                    literal.push(next);
                }
            }
            other => literal.push(other),
        }
    }
    flush_literal(&mut re, &mut literal);
    re.push('$');
    re
}

fn flush_literal(re: &mut String, literal: &mut String) {
    if !literal.is_empty() {
        re.push_str(&regex::escape(literal));
        literal.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matcher_last_rule_wins_with_negation() {
        let matcher = IgnoreMatcher::from_patterns(["Foo/**", "!Foo/keep"]);
        assert!(matcher.is_ignored("Foo/bar"));
        assert!(!matcher.is_ignored("Foo/keep"));
    }

    #[test]
    fn single_star_does_not_cross_separator() {
        let matcher = IgnoreMatcher::from_patterns([".claude/*.json"]);
        assert!(matcher.is_ignored(".claude/settings.json"));
        assert!(!matcher.is_ignored(".claude/nested/settings.json"));
    }

    #[test]
    fn comments_and_blank_lines_are_skipped() {
        let matcher = IgnoreMatcher::from_patterns(["# comment", "", "  ", ".secret"]);
        assert!(matcher.is_ignored(".secret"));
        assert!(!matcher.is_ignored("comment"));
    }

    #[test]
    fn double_star_crosses_separators() {
        let matcher = IgnoreMatcher::from_patterns(["Library/**"]);
        assert!(matcher.is_ignored("Library/Caches/x/y"));
        assert!(!matcher.is_ignored("Libraryish"));
    }

    #[test]
    fn recursive_pattern_collapses_the_directory_itself() {
        let matcher = IgnoreMatcher::from_patterns(["Library/**"]);
        // The bare directory does not match the pattern, but everything below
        // it does, so the directory as a whole is ignored.
        assert!(!matcher.is_ignored("Library"));
        assert!(matcher.dir_fully_ignored("Library"));
        assert!(!matcher.dir_fully_ignored("Documents"));
    }
}

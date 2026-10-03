//! Matching of destination paths against the rendered `.chezmoiignore` rules.
//!
//! `chezmoi unmanaged` applies `.chezmoiignore` itself, so the top level of the
//! Unmanaged view is ignore-correct. Expanding a directory in that view reads
//! children straight from the filesystem, which would otherwise surface files
//! chezmoi deliberately hides (including secrets that are ignored *because*
//! they are secret). This module provides the matcher used to re-apply the
//! ignore rules during that descent.

use anyhow::{Context, Result, bail};
use globset::GlobBuilder;
use regex::{Regex, RegexBuilder};

/// `.chezmoiignore` rules, with negations taking priority over all positive
/// rules regardless of their order, as in chezmoi's `PatternSet`.
pub struct IgnoreMatcher {
    rules: Vec<Rule>,
}

struct Rule {
    negated: bool,
    regex: Regex,
}

impl IgnoreMatcher {
    /// Compile rendered ignore lines. A malformed rule is an error, never a
    /// silently omitted filter. Comments start at `#` only at the beginning of
    /// a line or immediately after whitespace.
    pub fn from_patterns<I, S>(lines: I) -> Result<Self>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut rules = Vec::new();
        for (index, line) in lines.into_iter().enumerate() {
            let line = line.as_ref();
            let mut previous_whitespace = true;
            let comment = line.char_indices().find_map(|(index, ch)| {
                let is_comment = ch == '#' && previous_whitespace;
                // Go regexp's \s is ASCII (unlike Rust's Unicode \s).
                previous_whitespace = matches!(ch, ' ' | '\t' | '\n' | '\r' | '\u{c}');
                is_comment.then_some(index)
            });
            let raw = line[..comment.unwrap_or(line.len())].trim();
            if raw.is_empty() {
                continue;
            }
            let (negated, pattern) = match raw.strip_prefix('!') {
                Some(rest) => (true, rest),
                None => (false, raw),
            };
            let regex = compile_pattern(pattern).with_context(|| {
                format!("invalid ignore pattern on line {}: {raw:?}", index + 1)
            })?;
            rules.push(Rule { negated, regex });
        }
        Ok(Self { rules })
    }

    /// Whether a `/`-separated, destination-relative path is ignored.
    pub fn is_ignored(&self, rel: &str) -> bool {
        !self
            .rules
            .iter()
            .any(|rule| rule.negated && rule.regex.is_match(rel))
            && self
                .rules
                .iter()
                .any(|rule| !rule.negated && rule.regex.is_match(rel))
    }
}

/// globset owns wildcard and alternative parsing. Its byte-oriented regex and
/// character-class escapes differ from doublestar, so adapt those representations
/// and make the recursive suffix optional (doublestar's `Foo/**` matches `Foo`).
fn compile_pattern(pattern: &str) -> Result<Regex> {
    // SourceState.addPatterns validates relative paths, then RelPath.JoinString
    // applies Go's path.Clean before handing the glob to PatternSet.
    if pattern.is_empty() || pattern.starts_with('/') || pattern.split('/').any(|part| part == "..")
    {
        bail!("ignore pattern must be a non-empty destination-relative path without '..'");
    }
    let pattern = pattern
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect::<Vec<_>>()
        .join("/");
    let pattern = if pattern.is_empty() { "." } else { &pattern };
    let (pattern, classes) = adapt_classes(pattern)?;
    // Validate with globset before brace-expand, whose parser assumes balanced
    // braces. Expand first: doublestar interprets ** in the resulting component
    // context, while globset interprets it at each alternative's boundary.
    build_glob(&pattern)?;
    let patterns = expand_recursive_alternatives(&pattern)?;
    let suffix = Regex::new(r"/\.\*([)|$])")?;
    let mut regexes = Vec::new();
    for pattern in patterns {
        let glob = build_glob(&pattern)?;
        let mut regex = unicode_regex(glob.regex().trim_start_matches("(?-u)"))?;
        for (placeholder, class) in &classes {
            regex = regex.replace(placeholder, class);
        }
        regexes.push(suffix.replace_all(&regex, "(?:/.*)?$1").into_owned());
    }
    Ok(RegexBuilder::new(&regexes.join("|"))
        .dot_matches_new_line(true)
        .build()?)
}

fn build_glob(pattern: &str) -> Result<globset::Glob> {
    Ok(GlobBuilder::new(pattern)
        .literal_separator(true)
        .backslash_escape(true)
        .empty_alternates(true)
        .build()?)
}

fn expand_recursive_alternatives(pattern: &str) -> Result<Vec<String>> {
    if !pattern.contains("**") || !pattern.contains('{') {
        return Ok(vec![pattern.to_owned()]);
    }
    let mut prefix = String::from("__chezmoi_escape_");
    while pattern.contains(&prefix) {
        prefix.push('_');
    }
    let mut protected = String::new();
    let mut escapes = Vec::new();
    let mut chars = pattern.chars();
    // Product of sequential groups, sum of the current group's alternatives.
    let mut counts = vec![(1usize, 0usize)];
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            let next = chars.next().context("dangling escape")?;
            let placeholder = format!("{prefix}{}__", escapes.len());
            protected.push_str(&placeholder);
            escapes.push((placeholder, format!("\\{next}")));
        } else {
            match ch {
                '{' => counts.push((1, 0)),
                ',' if counts.len() > 1 => {
                    let (product, sum) = counts.last_mut().expect("outer group");
                    *sum += *product;
                    *product = 1;
                }
                '}' => {
                    let (product, sum) = counts.pop().context("unbalanced alternative")?;
                    counts.last_mut().context("unbalanced alternative")?.0 *= product + sum;
                }
                _ => {}
            }
            if counts
                .last()
                .is_some_and(|(product, sum)| product + sum > 1024)
            {
                bail!("recursive alternatives exceed the 1024-variant compilation limit");
            }
            protected.push(ch);
        }
    }
    // Bound expansion before the library allocates it, rather than allowing
    // exponential memory use.
    Ok(brace_expand::brace_expand(&protected)
        .into_iter()
        .map(|mut pattern| {
            for (placeholder, escaped) in &escapes {
                pattern = pattern.replace(placeholder, escaped);
            }
            pattern
        })
        .collect())
}

/// Preserve doublestar's escaped class members with regex's established class
/// parser; globset treats backslashes inside classes as ordinary characters.
fn adapt_classes(pattern: &str) -> Result<(String, Vec<(String, String)>)> {
    let mut adapted = String::new();
    let mut classes = Vec::new();
    let mut chars = pattern.chars().peekable();
    let mut prefix = String::from("__chezmoi_class_");
    while pattern.contains(&prefix) {
        prefix.push('_');
    }
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => {
                adapted.push(ch);
                adapted.push(chars.next().context("dangling escape")?);
            }
            '[' => {
                let negated = matches!(chars.peek(), Some('!' | '^'));
                if negated {
                    chars.next();
                }
                let mut members = Vec::new();
                loop {
                    match chars.next().context("unclosed character class")? {
                        ']' => break,
                        '\\' => {
                            members.push((chars.next().context("dangling class escape")?, true))
                        }
                        member => members.push((member, false)),
                    }
                }
                if members.is_empty() {
                    bail!("empty character class");
                }
                let mut class = String::from(if negated { "[^" } else { "[" });
                let mut index = 0;
                while index < members.len() {
                    let member = members[index].0;
                    class.push_str(&regex::escape(&member.to_string()));
                    if member != char::MAX
                        && members.get(index + 1) == Some(&('-', false))
                        && let Some(&(end, _)) = members.get(index + 2)
                    {
                        // doublestar checks a range's first rune literally
                        // before testing its range; a reversed range matches
                        // that first rune, not an invalid-regex error.
                        if member <= end {
                            class.push('-');
                            class.push_str(&regex::escape(&end.to_string()));
                        }
                        index += 3;
                    } else {
                        index += 1;
                    }
                }
                class.push(']');
                Regex::new(&class).context("invalid character class")?;
                let placeholder = format!("{prefix}{}__", classes.len());
                adapted.push_str(&placeholder);
                classes.push((placeholder, class));
            }
            _ => adapted.push(ch),
        }
    }
    Ok((adapted, classes))
}

/// Decode globset's UTF-8 byte literals before enabling Unicode matching, so
/// `?` and character ranges match runes, not individual UTF-8 bytes.
fn unicode_regex(regex: &str) -> Result<String> {
    let mut result = String::new();
    let mut chars = regex.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\\' || chars.peek() != Some(&'x') {
            result.push(ch);
            if ch == '\\' {
                result.push(chars.next().context("dangling regex escape")?);
            }
            continue;
        }
        let mut bytes = Vec::new();
        loop {
            chars.next(); // x
            let high = chars
                .next()
                .and_then(|ch| ch.to_digit(16))
                .context("invalid byte literal")?;
            let low = chars
                .next()
                .and_then(|ch| ch.to_digit(16))
                .context("invalid byte literal")?;
            bytes.push((high * 16 + low) as u8);
            let mut next = chars.clone();
            if next.next() != Some('\\') || next.next() != Some('x') {
                break;
            }
            chars.next(); // backslash
        }
        result.push_str(&regex::escape(std::str::from_utf8(&bytes)?));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compile_rules<I, S>(lines: I) -> IgnoreMatcher
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        IgnoreMatcher::from_patterns(lines).expect("valid test patterns")
    }

    #[test]
    fn recursive_alternative_expansion_is_bounded_without_rejecting_small_lists() {
        let pattern = format!(
            "Foo/{{**,{} }}",
            (0..12).map(|i| i.to_string()).collect::<Vec<_>>().join(",")
        );
        assert!(IgnoreMatcher::from_patterns([pattern]).is_ok());
        let pattern = format!("Foo/**/{}", "{a,b}".repeat(11));
        assert!(IgnoreMatcher::from_patterns([pattern]).is_err());
    }

    #[test]
    fn go_comment_whitespace_and_reversed_ranges_are_preserved() {
        let matcher = compile_rules(["a\u{3000}#b", "[z-a].token"]);
        assert!(matcher.is_ignored("a\u{3000}#b"));
        assert!(matcher.is_ignored("z.token"));
        assert!(!matcher.is_ignored("a.token"));
    }

    #[test]
    fn recursive_wildcards_in_alternatives_keep_their_component_context() {
        for (pattern, yes, no) in [
            ("Foo/{**,other}", "Foo/nested/secret", "Foobar"),
            ("{**,other}/secret", "nested/secret", "nested/public"),
            ("pre{**,other}post", "premiddlepost", "pre/deep/post"),
            (r"Foo/{**,literal\*}", "Foo", "Foobar"),
        ] {
            let matcher = compile_rules([pattern]);
            assert!(matcher.is_ignored(yes), "{pattern:?}: {yes:?}");
            assert!(!matcher.is_ignored(no), "{pattern:?}: {no:?}");
        }
    }

    #[test]
    fn rendered_paths_are_cleaned_as_chezmoi_joins_them() {
        let matcher = compile_rules(["backups/", "./secret.token", "cache//private"]);
        for path in ["backups", "secret.token", "cache/private"] {
            assert!(matcher.is_ignored(path), "{path:?}");
        }
        assert!(!matcher.is_ignored("backups/keep"));
    }

    #[test]
    fn invalid_relative_paths_are_errors() {
        for pattern in ["!", "../secret", "Foo/../secret", "/secret"] {
            assert!(
                IgnoreMatcher::from_patterns([pattern]).is_err(),
                "{pattern:?}"
            );
        }
    }

    #[test]
    fn unicode_wildcards_and_escaped_classes_follow_doublestar() {
        for (pattern, yes, no) in [
            ("?.token", "鍵.token", "鍵鍵.token"),
            ("[あ-お].token", "え.token", "か.token"),
            (r"[\!].token", "!.token", r"\.token"),
            (r"[a\-z].token", "-.token", "m.token"),
            (r"[\]].token", "].token", r"\.token"),
            ("{a,{b,c}}.token", "c.token", "d.token"),
            ("{,secret}.token", ".token", "public.token"),
        ] {
            let matcher = compile_rules([pattern]);
            assert!(matcher.is_ignored(yes), "{pattern:?}: {yes:?}");
            assert!(!matcher.is_ignored(no), "{pattern:?}: {no:?}");
        }
    }

    #[test]
    fn invalid_patterns_are_reported_with_line_context() {
        for pattern in ["[", "{secret,private", "dangling\\"] {
            let error = IgnoreMatcher::from_patterns(["# comment", pattern])
                .err()
                .expect("invalid glob");
            assert!(error.to_string().contains("line 2"));
        }
    }

    #[test]
    fn doublestar_matches_zero_components_and_embedded_stars_are_not_recursive() {
        let matcher = compile_rules(["**/*.token"]);
        assert!(matcher.is_ignored("secret.token"));
        assert!(matcher.is_ignored("a/b/secret.token"));
        let matcher = compile_rules(["Foo/**"]);
        assert!(matcher.is_ignored("Foo"));
        assert!(matcher.is_ignored("Foo/secret"));
        assert!(!matcher.is_ignored("Foobar"));
        let matcher = compile_rules(["Foo/pre**post"]);
        assert!(matcher.is_ignored("Foo/pre-middle-post"));
        assert!(!matcher.is_ignored("Foo/pre/deep/post"));
    }

    #[test]
    fn classes_alternatives_and_escapes_match_destination_names() {
        for (pattern, yes, no) in [
            ("[a-z].token", "s.token", "7.token"),
            ("[!x].token", "s.token", "x.token"),
            ("[^x].token", "s.token", "x.token"),
            ("{secret,private}.token", "secret.token", "public.token"),
            (r"literal\*.token", "literal*.token", "literalX.token"),
            (r"\!private", "!private", "private"),
        ] {
            let matcher = compile_rules([pattern]);
            assert!(matcher.is_ignored(yes), "{pattern:?}: {yes:?}");
            assert!(!matcher.is_ignored(no), "{pattern:?}: {no:?}");
        }
    }

    #[test]
    fn inline_comments_require_preceding_whitespace() {
        let matcher = compile_rules([
            "secret.token  # hide this file",
            "  # full comment",
            "backup.org#",
            "tab.token\t# comment",
            r"literal\#name",
            "trim.token  ",
        ]);
        for path in [
            "secret.token",
            "backup.org#",
            "tab.token",
            "literal#name",
            "trim.token",
        ] {
            assert!(matcher.is_ignored(path), "{path:?}");
        }
        assert!(!matcher.is_ignored("backup.org"));
    }

    #[test]
    fn negations_take_precedence_regardless_of_order() {
        for patterns in [["Foo/**", "!Foo/keep"], ["!Foo/keep", "Foo/**"]] {
            let matcher = compile_rules(patterns);
            assert!(!matcher.is_ignored("Foo/keep"), "{patterns:?}");
            assert!(matcher.is_ignored("Foo/secret"));
        }
    }

    #[test]
    fn negation_reincludes_an_ignored_child() {
        let matcher = compile_rules(["Foo/**", "!Foo/keep"]);
        assert!(matcher.is_ignored("Foo/bar"));
        assert!(!matcher.is_ignored("Foo/keep"));
    }

    #[test]
    fn single_star_does_not_cross_separator() {
        let matcher = compile_rules([".claude/*.json"]);
        assert!(matcher.is_ignored(".claude/settings.json"));
        assert!(!matcher.is_ignored(".claude/nested/settings.json"));
    }

    #[test]
    fn comments_and_blank_lines_are_skipped() {
        let matcher = compile_rules(["# comment", "", "  ", ".secret"]);
        assert!(matcher.is_ignored(".secret"));
        assert!(!matcher.is_ignored("comment"));
    }

    #[test]
    fn double_star_crosses_separators() {
        let matcher = compile_rules(["Library/**"]);
        assert!(matcher.is_ignored("Library/Caches/x/y"));
        assert!(!matcher.is_ignored("Libraryish"));
    }

    #[test]
    fn recursive_pattern_collapses_the_directory_itself() {
        let matcher = compile_rules(["Library/**"]);
        assert!(matcher.is_ignored("Library"));
        assert!(!matcher.is_ignored("Documents"));
    }
}

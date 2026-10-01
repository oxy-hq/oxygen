//! Which secret keys a function's bundled JS writes — the string-literal first
//! argument of every `ctx.secrets.set(…)`, however the property chain is
//! spelled. The same forms `oxyc validate` reads
//! (`sdk/cli/src/publish/shared-env.ts`), so the CLI's advice and this gate
//! agree:
//!
//! - `….secrets.set(` and `….secrets?.set(` (minified: `e.secrets.set(`);
//! - `…["secrets"].set(` (any quote);
//! - a destructured alias, `const { secrets: s } = ctx; s.set(` (minified:
//!   `{secrets:s}`), and plain `{ secrets }`, which the first form reads.
//!
//! APPROXIMATE on purpose, in the refusing direction: comments and strings are
//! not masked, so a commented-out call still counts, and an alias is matched by
//! name wherever it appears. A key computed at runtime is not seen at all; the
//! runtime backstop is `ctx.secrets.set` refusing a key read through the
//! shared fallback.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use regex::Regex;

/// `secrets.set(`, `secrets?.set(` and `["secrets"].set(`, ending at the `(`.
static DIRECT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?:\bsecrets\s*\??\.|\[\s*["'`]secrets["'`]\s*\]\s*\??\.)\s*set\s*\("#)
        .expect("valid regex")
});

/// `{ secrets: alias` / `, secrets: alias` — a destructuring rename.
static ALIAS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[{,]\s*secrets\s*:\s*([A-Za-z_$][A-Za-z0-9_$]*)").expect("valid regex")
});

/// Every string-literal key `source` passes to `ctx.secrets.set`.
pub(crate) fn written_secret_keys(source: &str) -> BTreeSet<String> {
    let mut calls: Vec<usize> = DIRECT.find_iter(source).map(|m| m.end()).collect();
    let aliases: BTreeSet<&str> = ALIAS
        .captures_iter(source)
        .filter_map(|c| c.get(1).map(|m| m.as_str()))
        .collect();
    for alias in aliases {
        let pattern = format!(
            r"(?:^|[^A-Za-z0-9_$.]){}\s*\??\.\s*set\s*\(",
            regex::escape(alias)
        );
        if let Ok(re) = Regex::new(&pattern) {
            calls.extend(re.find_iter(source).map(|m| m.end()));
        }
    }
    calls
        .into_iter()
        .filter_map(|open| literal_first_arg(&source[open..]))
        .collect()
}

/// The first argument of a call whose `(` was just consumed, when it is a
/// plain string literal: `"K"`, `'K'`, or `` `K` `` with no interpolation.
fn literal_first_arg(args: &str) -> Option<String> {
    let args = args.trim_start();
    let quote = args
        .chars()
        .next()
        .filter(|c| matches!(c, '"' | '\'' | '`'))?;
    let rest = &args[1..];
    let end = rest.find(quote)?;
    let key = &rest[..end];
    let interpolated = quote == '`' && key.contains("${");
    (!key.is_empty() && !key.contains('\\') && !interpolated).then(|| key.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(source: &str) -> Vec<String> {
        written_secret_keys(source).into_iter().collect()
    }

    #[test]
    fn every_spelling_of_the_call_is_read() {
        assert_eq!(keys(r#"await ctx.secrets.set("A", v)"#), ["A"]);
        assert_eq!(keys(r#"e.secrets.set('B',t)"#), ["B"]);
        assert_eq!(keys("ctx.secrets?.set(`C`, v)"), ["C"]);
        assert_eq!(keys(r#"ctx . secrets . set ( "D" , v)"#), ["D"]);
        assert_eq!(keys(r#"ctx["secrets"].set("E", v)"#), ["E"]);
        assert_eq!(keys(r#"ctx['secrets']?.set('F', v)"#), ["F"]);
    }

    #[test]
    fn a_destructured_alias_is_followed() {
        let minified = r#"async(r,c)=>{const{secrets:s,env:n}=c;await s.set("G",n.X)}"#;
        assert_eq!(keys(minified), ["G"]);
        let spaced = "const { env, secrets: store } = ctx;\nawait store?.set('H', v);";
        assert_eq!(keys(spaced), ["H"]);
        let plain = "const { secrets } = ctx; secrets.set(\"I\", v);";
        assert_eq!(keys(plain), ["I"]);
    }

    #[test]
    fn a_computed_key_or_another_receiver_is_not_a_write() {
        assert!(keys("ctx.secrets.set(keyFor(org), v)").is_empty());
        assert!(keys("ctx.secrets.set(`K_${org}`, v)").is_empty());
        assert!(keys(r#"cache.set("K", v); ctx.env.K"#).is_empty());
        // An alias is only its own name: `x.s.set(` is a property, not `s`.
        assert!(keys(r#"const{secrets:s}=c; x.s.set("K", v)"#).is_empty());
    }

    #[test]
    fn a_key_that_only_prefixes_a_written_key_is_not_written() {
        assert_eq!(keys("ctx.secrets.set('QB_TOKEN_2', v)"), ["QB_TOKEN_2"]);
        assert!(!written_secret_keys("ctx.secrets.set('QB_TOKEN_2', v)").contains("QB_TOKEN"));
    }
}

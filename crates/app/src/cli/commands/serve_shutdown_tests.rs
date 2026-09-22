//! Guards for what the server's shutdown path may do to Docker. Source scans
//! only — nothing here builds a Docker client or needs a container runtime.
//!
//! Co-located via `#[path = "serve_shutdown_tests.rs"] mod shutdown_tests;` from
//! `serve.rs`, which is far past the file-size limit already.
//!
//! The database containers carry fixed names, and both `oxy serve` and
//! `oxy start` shut down through `serve.rs`. It used to end with an
//! unconditional `docker::cleanup_containers()`, so stopping a second local
//! `oxy serve` removed the running `oxy start` stack's `oxy-postgres` and
//! `oxy-clickhouse`. The behaviour itself is unit-tested next to the code, in
//! `oxy::database::docker::ownership`; these scans pin the call sites, which
//! that test cannot see.
//!
//! Matching is on source with `//` comments — whole-line and trailing — dropped
//! and all whitespace removed, so reformatting passes and a comment naming a
//! symbol cannot satisfy (or trip) an assertion.

/// `src` with `//` comments — whole-line and trailing — and every whitespace
/// character removed. A `//` inside a string literal (a URL) is kept: string
/// state is tracked across lines, since `serve.rs` has messages that open on one
/// line and carry `postgresql://…` on the next. Assumes what holds for the files
/// scanned here: no block comments and no raw string ending in a backslash. Twin
/// of `code` in `crates/core/src/database/docker/ownership.rs`.
fn code(src: &str) -> String {
    let chars: Vec<char> = src.chars().collect();
    let mut out = String::with_capacity(src.len());
    let (mut i, mut in_string) = (0, false);
    while i < chars.len() {
        let (c, next) = (chars[i], chars.get(i + 1).copied());
        if in_string && c == '\\' {
            // An escape: keep both characters, so `\"` cannot close the string.
            out.push(c);
            out.extend(next.filter(|n| !n.is_whitespace()));
            i += 2;
        } else if !in_string && c == '/' && next == Some('/') {
            // A comment runs to the end of its line, quotes and apostrophes included.
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else if !in_string && c == '\'' && next == Some('"') {
            // The char literal `'"'` is not a string delimiter.
            out.push_str("'\"");
            i += 2;
        } else {
            in_string ^= c == '"';
            if !c.is_whitespace() {
                out.push(c);
            }
            i += 1;
        }
    }
    assert!(!in_string, "the scan lost track of string literals");
    out
}

#[test]
fn the_scan_drops_comments_but_keeps_a_double_slash_inside_a_string() {
    assert_eq!(code("a(); // b()\n// c()\nd();"), "a();d();");
    assert_eq!(code("x(\"http://h\"); // y()"), "x(\"http://h\");");
    assert_eq!(
        code("m(\"one\\n\\\n   pg://u\"); z();"),
        "m(\"one\\n\\pg://u\");z();"
    );
    assert_eq!(code("k = 1; // \"\\0xy\" don't\nn();"), "k=1;n();");
    assert_eq!(code("q('\"'); // r()\ns();"), "q('\"');s();");
}

/// The body of the first `fn <name>(` in squashed source, by brace matching.
/// Braces inside string literals are counted too, so this assumes they balance
/// there: `"{}"` does; a lone `"{{"` gives a wrong body or "unbalanced braces".
fn fn_body<'a>(src: &'a str, name: &str) -> &'a str {
    let sig = format!("fn{name}(");
    let at = src
        .find(&sig)
        .unwrap_or_else(|| panic!("`fn {name}` not found"));
    let open = at + src[at..].find('{').expect("a function body");
    let mut depth = 0;
    for (i, c) in src[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' if depth == 1 => return &src[open..=open + i],
            '}' => depth -= 1,
            _ => {}
        }
    }
    panic!("unbalanced braces in `fn {name}`")
}

/// The docker-module entry points that remove containers BY NAME, whoever
/// created them. Neither belongs on a shutdown path. `clean_all` is matched
/// module-qualified because `cli::commands::clean` has an unrelated `clean_all`.
const REMOVES_BY_NAME: [&str; 2] = ["cleanup_containers(", "docker::clean_all("];

#[test]
fn serve_never_removes_containers_by_name() {
    let serve = code(include_str!("serve.rs"));
    for call in REMOVES_BY_NAME {
        assert!(
            !serve.contains(call),
            "`serve.rs` calls `{call}…)` again. Every `oxy serve` shuts down through this \
             file, and that call removes `oxy-postgres` / `oxy-clickhouse` by their fixed \
             names — including a stack another process started. Shutdown must go through \
             `docker::cleanup_owned_containers()`, which removes only what this process created."
        );
    }
}

#[test]
fn shutdown_still_removes_the_containers_this_process_created() {
    let serve = code(include_str!("serve.rs"));
    assert!(
        fn_body(&serve, "create_shutdown_signal")
            .contains("docker::cleanup_owned_containers().await"),
        "`create_shutdown_signal` no longer runs `docker::cleanup_owned_containers()`, so \
         `oxy start` would leave its containers running after Ctrl-C"
    );
}

#[test]
fn oxy_start_still_clears_leftover_containers_before_creating_its_own() {
    let start = code(include_str!("start.rs"));
    let body = fn_body(&start, "start_database_and_server");
    let cleared = body
        .find("docker::cleanup_containers().await")
        .expect("`oxy start` no longer clears leftover containers on startup");
    let created = body
        .find("start_postgres().await")
        .expect("`oxy start` no longer starts Postgres");
    assert!(
        cleared < created,
        "`oxy start` must clear leftover containers BEFORE creating its own: \
         `start_postgres_container` always creates, and fails on a name already taken"
    );
}

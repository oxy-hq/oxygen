//! Whether an archive member's path stays inside the directory it unpacks
//! into. Two readers of untrusted archives apply it: a custom-app bundle's
//! publish (`oxy-api-custom-apps`) and a compile's commit tarball
//! (`oxy-compile-runtime`'s `git::unpack`). Down from `oxy_custom_apps::manifest`
//! (2026-11-26) so the compile runtime need not depend on custom apps;
//! re-exported there under its old name.

/// A tar entry path is safe iff it's relative and contains no `..`
/// component — guards against an archive member escaping the build
/// prefix (`../../etc/...`) when its files are later keyed into S3.
pub fn is_safe_relative_path(path: &std::path::Path) -> bool {
    !path.is_absolute()
        && !path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
}

#[cfg(test)]
mod tests {
    use super::is_safe_relative_path;
    use std::path::Path;

    #[test]
    fn relative_paths_without_parent_steps_are_safe() {
        assert!(is_safe_relative_path(Path::new("index.html")));
        assert!(is_safe_relative_path(Path::new("assets/app.js")));
        assert!(!is_safe_relative_path(Path::new("../escape.html")));
        assert!(!is_safe_relative_path(Path::new("a/../../b")));
        assert!(!is_safe_relative_path(Path::new("/abs/secret")));
    }
}

//! Confinement for request-body-supplied filesystem paths.
//!
//! Several endpoints accept a caller-supplied directory (download/transcode/scan
//! roots, migration target, folder browser). On a public deployment those must
//! not be able to point reads/writes at system locations (`/etc`, `/usr`, ...).
//! We confine them to a set of allowed roots: the user's home directory and
//! common media mount points that exist on the host.

use std::path::{Component, Path, PathBuf};

/// Directories that caller-supplied absolute paths are allowed to live under.
pub fn allowed_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = dirs::home_dir() {
        roots.push(home);
    }
    for p in ["/media", "/mnt", "/Volumes", "/data", "/srv"] {
        let p = PathBuf::from(p);
        if p.is_dir() {
            roots.push(p);
        }
    }
    roots
}

/// True when an already-canonical path lies inside one of the allowed roots.
/// Used by the folder browser to confine directory listings.
pub fn is_under_allowed_root(canonical: &Path) -> bool {
    under_allowed_root(canonical, &allowed_roots())
}

/// True when `canonical` is inside one of `roots` (each root canonicalized so
/// symlinked roots are compared by their real location).
fn under_allowed_root(canonical: &Path, roots: &[PathBuf]) -> bool {
    roots.iter().any(|root| {
        std::fs::canonicalize(root)
            .map(|rc| canonical.starts_with(&rc))
            .unwrap_or(false)
    })
}

/// Validate a configurable directory from a settings body. Relative paths (e.g.
/// the default `"."`) are allowed as long as they contain no `..` (so they can't
/// escape the working dir); absolute paths must resolve under an allowed root.
/// Empty is always rejected.
pub fn validate_config_dir(dir: &Path) -> Result<(), &'static str> {
    if dir.as_os_str().is_empty() {
        return Err("must not be empty");
    }
    if dir.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err("must not contain '..'");
    }
    if dir.is_absolute() {
        confine(dir).map(|_| ())
    } else {
        Ok(())
    }
}

/// Confine a caller-supplied absolute path (that may not exist yet, e.g. a
/// migration target) to the allowed roots. Rejects empty/relative/`..` paths,
/// then verifies the nearest existing ancestor (canonicalized, resolving
/// symlinks) lies under an allowed root. Returns the path unchanged on success.
pub fn confine(candidate: &Path) -> Result<PathBuf, &'static str> {
    if candidate.as_os_str().is_empty() {
        return Err("path is empty");
    }
    if !candidate.is_absolute() {
        return Err("path must be absolute");
    }
    if candidate
        .components()
        .any(|c| matches!(c, Component::ParentDir))
    {
        return Err("path must not contain '..'");
    }
    // Walk up to the nearest existing ancestor and canonicalize it. A path whose
    // real ancestor is under an allowed root, with no `..` in the tail, cannot
    // escape that root.
    let mut existing = candidate;
    loop {
        if existing.exists() {
            break;
        }
        match existing.parent() {
            Some(parent) => existing = parent,
            None => return Err("path has no existing ancestor"),
        }
    }
    let canonical_existing = std::fs::canonicalize(existing).map_err(|_| "cannot resolve path")?;
    if !under_allowed_root(&canonical_existing, &allowed_roots()) {
        return Err("path is outside the allowed media directories");
    }
    Ok(candidate.to_path_buf())
}

/// Confine an existing directory (e.g. a scan or browse target). Canonicalizes
/// the path itself and confirms it is a directory under an allowed root. Returns
/// the canonical path.
pub fn confine_existing_dir(candidate: &Path) -> Result<PathBuf, &'static str> {
    let canonical = std::fs::canonicalize(candidate).map_err(|_| "path not found")?;
    if !canonical.is_dir() {
        return Err("not a directory");
    }
    if !under_allowed_root(&canonical, &allowed_roots()) {
        return Err("path is outside the allowed media directories");
    }
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_absolute_system_paths() {
        assert!(confine(Path::new("/etc")).is_err());
        assert!(confine(Path::new("/usr/bin")).is_err());
    }

    #[test]
    fn rejects_empty_and_parent_dir() {
        assert!(validate_config_dir(Path::new("")).is_err());
        assert!(validate_config_dir(Path::new("../../etc")).is_err());
        assert!(confine(Path::new("/data/../etc")).is_err());
    }

    #[test]
    fn allows_relative_default() {
        // The default download_dir "." must stay valid.
        assert!(validate_config_dir(Path::new(".")).is_ok());
        assert!(validate_config_dir(Path::new("downloads")).is_ok());
    }

    #[test]
    fn allows_path_under_home() {
        if let Some(home) = dirs::home_dir() {
            let under = home.join(".movies").join("data");
            // Nearest existing ancestor is home (exists) → confined ok even if
            // the leaf doesn't exist.
            assert!(confine(&under).is_ok());
        }
    }
}

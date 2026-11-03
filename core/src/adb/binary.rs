//! Locating the `adb` executable.
//!
//! Every ADB call used to spawn `adb` and rely on it being on `PATH`. That is
//! true in the dev environment, where the SDK is added to `PATH` by hand, and
//! false on a user's machine — so a shipped app has to bundle platform-tools or
//! find an installed SDK.
//!
//! Resolution order is explicit and testable:
//!
//! 1. `AI_UPCYCLER_ADB` — an operator pointing at a specific binary.
//! 2. A bundled `platform-tools/` next to the executable.
//! 3. `adb` on `PATH`.
//!
//! The pure part ([`resolve_from`]) takes its inputs as arguments so the order
//! can be tested without touching process-global environment variables.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Environment variable that overrides discovery entirely.
pub const ADB_ENV: &str = "AI_UPCYCLER_ADB";

/// A resolved `adb` executable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdbBinary {
    path: PathBuf,
}

impl AdbBinary {
    /// Uses `path` as-is. Tests and callers that already know where adb lives
    /// should prefer this to the environment.
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Finds adb for this process: env override, bundled copy, then `PATH`.
    pub fn resolve() -> Result<Self, String> {
        let explicit = std::env::var_os(ADB_ENV).map(PathBuf::from);
        resolve_from(explicit, bundled_candidate(), path_candidate())
    }

    /// Runs adb with `args`, returning the raw output.
    pub fn run(&self, args: &[&str]) -> Result<Output, String> {
        Command::new(&self.path).args(args).output().map_err(|e| {
            format!(
                "failed to run {} {}: {e}",
                self.path.display(),
                args.join(" ")
            )
        })
    }

    /// Runs adb and returns stdout, treating a non-zero exit as an error.
    pub fn run_text(&self, args: &[&str]) -> Result<String, String> {
        let output = self.run(args)?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            Err(format!(
                "'adb {}' failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }
}

/// The resolution decision, with every input supplied by the caller.
///
/// Split out from [`AdbBinary::resolve`] so tests can exercise the precedence
/// without mutating the process environment, which is shared by every test in
/// the binary.
pub fn resolve_from(
    explicit: Option<PathBuf>,
    bundled: Option<PathBuf>,
    on_path: Option<PathBuf>,
) -> Result<AdbBinary, String> {
    if let Some(path) = explicit {
        if path.is_file() {
            return Ok(AdbBinary::at(path));
        }
        return Err(format!(
            "{ADB_ENV} points at {}, which is not a file",
            path.display()
        ));
    }
    if let Some(path) = bundled.filter(|candidate| candidate.is_file()) {
        return Ok(AdbBinary::at(path));
    }
    if let Some(path) = on_path {
        return Ok(AdbBinary::at(path));
    }
    Err(format!(
        "adb was not found: set {ADB_ENV}, bundle platform-tools next to the app, \
         or put adb on PATH"
    ))
}

/// `platform-tools/adb[.exe]` beside the running executable, which is where a
/// bundled Tauri resource lands.
fn bundled_candidate() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    Some(dir.join("platform-tools").join(adb_file_name()))
}

/// `adb` on `PATH`, if invoking it works.
///
/// `Command::new("adb")` is the portable way to ask the OS to search `PATH`;
/// spawning `version` also proves the binary actually runs, which a bare
/// `which`-style file check would not.
fn path_candidate() -> Option<PathBuf> {
    Command::new("adb")
        .arg("version")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|_| PathBuf::from("adb"))
}

fn adb_file_name() -> &'static str {
    if cfg!(windows) {
        "adb.exe"
    } else {
        "adb"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ai-upcycler-adb-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join(name);
        std::fs::write(&path, b"stub").expect("stub file");
        path
    }

    #[test]
    fn an_explicit_binary_wins_over_everything_else() {
        let explicit = temp_file("adb-explicit");
        let bundled = temp_file("adb-bundled");
        let chosen = resolve_from(
            Some(explicit.clone()),
            Some(bundled),
            Some(PathBuf::from("adb")),
        )
        .expect("explicit wins");
        assert_eq!(chosen.path(), explicit);
    }

    #[test]
    fn a_missing_explicit_binary_is_an_error_not_a_fallback() {
        // Silently falling through to PATH here would run a different adb than
        // the operator asked for, which is exactly the bug this prevents.
        let error = resolve_from(
            Some(PathBuf::from("C:/definitely/not/here/adb.exe")),
            Some(temp_file("adb-bundled")),
            Some(PathBuf::from("adb")),
        )
        .expect_err("must not fall back");
        assert!(error.contains(ADB_ENV), "{error}");
    }

    #[test]
    fn the_bundled_copy_is_preferred_over_path() {
        let bundled = temp_file("adb-bundled");
        let chosen = resolve_from(None, Some(bundled.clone()), Some(PathBuf::from("adb")))
            .expect("bundled wins");
        assert_eq!(chosen.path(), bundled);
    }

    #[test]
    fn path_is_the_last_resort() {
        let chosen = resolve_from(None, None, Some(PathBuf::from("adb"))).expect("path");
        assert_eq!(chosen.path(), Path::new("adb"));
    }

    #[test]
    fn nothing_found_explains_all_three_ways_to_fix_it() {
        let error = resolve_from(None, None, None).expect_err("nothing found");
        assert!(error.contains(ADB_ENV), "{error}");
        assert!(error.contains("platform-tools"), "{error}");
        assert!(error.contains("PATH"), "{error}");
    }

    #[test]
    fn a_bundled_candidate_that_does_not_exist_is_skipped() {
        let chosen = resolve_from(
            None,
            Some(PathBuf::from("C:/definitely/not/here/adb.exe")),
            Some(PathBuf::from("adb")),
        )
        .expect("falls through to PATH");
        assert_eq!(chosen.path(), Path::new("adb"));
    }
}

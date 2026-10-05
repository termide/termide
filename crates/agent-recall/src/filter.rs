//! The narrowing a search may ask for: project paths and a start date.

use std::path::{Component, Path, PathBuf};

/// `paths` as the model gives them, relative to `cwd` or absolute, made
/// relative to `project_root` as [`PathFilter`] takes them. One outside the
/// project is kept as given, so it matches nothing rather than something else.
#[must_use]
pub fn project_paths(paths: &[String], cwd: &Path, project_root: &Path) -> Vec<String> {
    paths
        .iter()
        .map(|path| {
            let full = normalize(&cwd.join(path.trim()));
            match full.strip_prefix(project_root) {
                Ok(inside) => inside.to_string_lossy().replace('\\', "/"),
                Err(_) => path.clone(),
            }
        })
        .collect()
}

/// The project-relative `path` as seen from `cwd`: relative under it,
/// absolute elsewhere, so the read tool finds it from there.
#[must_use]
pub fn path_from_cwd(path: &str, cwd: &Path, project_root: &Path) -> String {
    let full = project_root.join(path);
    match full.strip_prefix(cwd) {
        Ok(inside) if !inside.as_os_str().is_empty() => inside.to_string_lossy().replace('\\', "/"),
        _ => full.to_string_lossy().into_owned(),
    }
}

/// `path` without `.` and `..` components, resolved lexically.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// The `paths` a search is narrowed to, relative to the project root: a
/// plain path covers itself and everything under it, a glob (`*`, `?`, `[`)
/// matches as `glob` does, `**` crossing directories.
#[derive(Debug, Clone, Default)]
pub struct PathFilter {
    items: Vec<PathItem>,
}

#[derive(Debug, Clone)]
enum PathItem {
    Plain(String),
    Glob(glob::Pattern),
}

impl PathFilter {
    /// The filter for `paths`; a pattern that does not parse as a glob is
    /// taken as a plain path.
    #[must_use]
    pub fn new(paths: &[String]) -> Self {
        let items = paths
            .iter()
            .map(|path| path.trim().trim_start_matches("./").trim_end_matches('/'))
            .filter(|path| !path.is_empty())
            .map(|path| {
                if path.contains(['*', '?', '[']) {
                    if let Ok(pattern) = glob::Pattern::new(path) {
                        return PathItem::Glob(pattern);
                    }
                }
                PathItem::Plain(path.to_string())
            })
            .collect();
        Self { items }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Whether the project-relative `path` is covered.
    #[must_use]
    pub fn matches(&self, path: &str) -> bool {
        let path = path.trim_start_matches("./");
        self.items.iter().any(|item| match item {
            PathItem::Plain(plain) => {
                path == plain
                    || path
                        .strip_prefix(plain.as_str())
                        .is_some_and(|rest| rest.starts_with('/'))
            }
            PathItem::Glob(pattern) => {
                let options = glob::MatchOptions {
                    require_literal_separator: true,
                    ..glob::MatchOptions::new()
                };
                pattern.matches_with(path, options)
                    || Path::new(path)
                        .ancestors()
                        .skip(1)
                        .filter_map(|dir| dir.to_str())
                        .any(|dir| !dir.is_empty() && pattern.matches_with(dir, options))
            }
        })
    }

    /// Whether `text` names one of the plain paths, as a session message
    /// that discusses a file without calling a tool on it does.
    #[must_use]
    pub fn mentioned_in(&self, text: &str) -> bool {
        self.items.iter().any(|item| match item {
            PathItem::Plain(plain) => text.contains(plain.as_str()),
            PathItem::Glob(_) => false,
        })
    }

    /// What the filter selects inside the project directory `dir` (a nested
    /// repository; empty for the project root), relative to `dir`.
    #[must_use]
    pub fn within(&self, dir: &str) -> Scope {
        if self.items.is_empty() {
            return Scope::All;
        }
        let dir = dir.trim_start_matches("./").trim_end_matches('/');
        let mut specs = Vec::new();
        for item in &self.items {
            match item {
                PathItem::Plain(plain) => {
                    if dir.is_empty() {
                        specs.push(Spec::Plain(plain.clone()));
                    } else if plain == dir || dir.starts_with(&format!("{plain}/")) {
                        return Scope::All;
                    } else if let Some(rest) = plain.strip_prefix(&format!("{dir}/")) {
                        specs.push(Spec::Plain(rest.to_string()));
                    }
                }
                PathItem::Glob(pattern) => {
                    let text = pattern.as_str();
                    if dir.is_empty() || text.starts_with("**/") {
                        specs.push(Spec::Glob(text.to_string()));
                    } else if let Some(rest) = text.strip_prefix(&format!("{dir}/")) {
                        specs.push(Spec::Glob(rest.to_string()));
                    }
                }
            }
        }
        if specs.is_empty() {
            Scope::Nothing
        } else {
            Scope::Some(specs)
        }
    }
}

/// What a path filter selects inside one directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// Everything: no filter, or one that covers the whole directory.
    All,
    /// These paths, relative to the directory.
    Some(Vec<Spec>),
    /// Nothing in it.
    Nothing,
}

/// One path of a [`Scope`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Spec {
    Plain(String),
    Glob(String),
}

impl Spec {
    /// As a git pathspec under `base` (the project's directory inside the
    /// repository, empty at its root); a glob gets git's `glob` magic, so
    /// `**` crosses directories as it does here.
    #[must_use]
    pub fn pathspec(&self, base: &Path) -> String {
        match self {
            Self::Plain(plain) => base.join(plain).to_string_lossy().into_owned(),
            Self::Glob(glob) => format!(":(glob){}", base.join(glob).to_string_lossy()),
        }
    }

    /// As text for [`PathFilter::new`].
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Plain(text) | Self::Glob(text) => text,
        }
    }
}

/// `YYYY-MM-DD` as milliseconds since the Unix epoch at 00:00 UTC.
///
/// # Errors
///
/// When the text is not such a date.
pub fn parse_date(text: &str) -> Result<u64, String> {
    let invalid = || format!("`{text}` is not a date in the form YYYY-MM-DD");
    let mut fields = text.trim().splitn(3, '-');
    let mut next = || -> Result<i64, String> {
        fields
            .next()
            .and_then(|field| field.parse().ok())
            .ok_or_else(invalid)
    };
    let (year, month, day) = (next()?, next()?, next()?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || year < 1970 {
        return Err(invalid());
    }
    // Days from the civil date, the inverse of `civil_date` (Howard
    // Hinnant's algorithm).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days * 86_400_000).map_err(|_| invalid())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_read_from_the_working_directory_and_results_point_back_to_it() {
        let root = Path::new("/p");
        let cwd = Path::new("/p/crates");
        let given = [
            "core/src".to_string(),
            "../doc/*.md".to_string(),
            "/p/README.md".to_string(),
            "/elsewhere/x".to_string(),
            ".".to_string(),
        ];
        assert_eq!(
            project_paths(&given, cwd, root),
            [
                "crates/core/src",
                "doc/*.md",
                "README.md",
                "/elsewhere/x",
                "crates"
            ]
        );
        assert_eq!(project_paths(&given[..1], root, root), ["core/src"]);

        assert_eq!(path_from_cwd("crates/core/x.rs", cwd, root), "core/x.rs");
        assert_eq!(path_from_cwd("doc/a.md", cwd, root), "/p/doc/a.md");
        assert_eq!(path_from_cwd("doc/a.md", root, root), "doc/a.md");
    }

    #[test]
    fn plain_paths_cover_their_subtree_and_globs_match() {
        let filter = PathFilter::new(&[
            "crates/agent-core/".to_string(),
            "src/**/*.rs".to_string(),
            "doc/*/agent.md".to_string(),
        ]);
        assert!(filter.matches("crates/agent-core"));
        assert!(filter.matches("crates/agent-core/src/layers.rs"));
        assert!(!filter.matches("crates/agent-core-x/lib.rs"));
        assert!(filter.matches("src/app/ui.rs"));
        assert!(filter.matches("doc/ru/agent.md"));
        assert!(!filter.matches("doc/ru/x/agent.md"));
        assert!(filter.mentioned_in("see crates/agent-core/src/acp.rs"));
        assert!(PathFilter::new(&[" ".to_string()]).is_empty());
    }

    #[test]
    fn a_filter_is_restated_inside_a_nested_directory() {
        let filter = PathFilter::new(&[
            "repos/a/src".to_string(),
            "**/*.md".to_string(),
            "repos/b/*.rs".to_string(),
        ]);
        assert_eq!(
            filter.within("repos/a"),
            Scope::Some(vec![
                Spec::Plain("src".into()),
                Spec::Glob("**/*.md".into())
            ])
        );
        assert_eq!(
            filter.within("repos/b"),
            Scope::Some(vec![
                Spec::Glob("**/*.md".into()),
                Spec::Glob("*.rs".into())
            ])
        );
        assert_eq!(
            PathFilter::new(&["repos".to_string()]).within("repos/a"),
            Scope::All
        );
        assert_eq!(
            PathFilter::new(&["docs".to_string()]).within("repos/a"),
            Scope::Nothing
        );
        assert_eq!(PathFilter::default().within("x"), Scope::All);
        assert_eq!(
            Spec::Plain("src".into()).pathspec(Path::new("proj")),
            "proj/src"
        );
        assert_eq!(
            Spec::Glob("**/*.md".into()).pathspec(Path::new("")),
            ":(glob)**/*.md"
        );
    }

    #[test]
    fn dates_round_trip_with_civil_date() {
        let millis = parse_date("2026-10-05").unwrap();
        assert_eq!(termide_agent_core::civil_date(millis), "2026-10-05");
        assert_eq!(parse_date("1970-01-01"), Ok(0));
        assert!(parse_date("2026-13-01").is_err());
        assert!(parse_date("yesterday").is_err());
    }
}

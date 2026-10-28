//! In-memory table of contents of an opened archive.
//!
//! Entry names are untrusted input: every name is normalised into a path
//! of plain components before it enters the index, and anything that could
//! escape the archive root (`..`, a drive prefix, an embedded separator) is
//! dropped. Everything downstream — listing, reading, extraction — only ever
//! sees these normalised paths.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use crate::types::VfsMetadata;

/// Where an entry's bytes live inside the archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum EntrySource {
    /// Position in the zip central directory.
    Zip(usize),
    /// Ordinal of the entry in the tar stream.
    Tar(usize),
    /// Position in the extent lists of an ISO image.
    Iso(usize),
}

#[derive(Debug, Clone)]
pub(crate) enum NodeKind {
    Dir,
    File(EntrySource),
    /// The link target exactly as stored in the archive.
    Symlink(String),
    /// A tar hard link; the target is an index key.
    HardLink(PathBuf),
}

#[derive(Debug, Clone)]
pub(crate) struct Node {
    pub(crate) kind: NodeKind,
    pub(crate) meta: VfsMetadata,
}

/// How many links a lookup follows before giving up on a loop.
const MAX_LINK_HOPS: usize = 40;

/// Table of contents keyed by absolute inner path (`/` is the root).
#[derive(Debug)]
pub(crate) struct ArchiveIndex {
    nodes: BTreeMap<PathBuf, Node>,
    children: BTreeMap<PathBuf, BTreeSet<String>>,
}

impl ArchiveIndex {
    pub(crate) fn new() -> Self {
        let mut index = Self {
            nodes: BTreeMap::new(),
            children: BTreeMap::new(),
        };
        index
            .nodes
            .insert(root(), dir_node(VfsMetadata::directory()));
        index.children.insert(root(), BTreeSet::new());
        index
    }

    /// Add an entry under its raw archive name. Returns false when the name
    /// is unsafe or collides with the existing tree, in which case the entry
    /// is not browsable. A later entry with the same name replaces an earlier
    /// one, matching how tar appends updates.
    pub(crate) fn insert(&mut self, raw_name: &str, kind: NodeKind, meta: VfsMetadata) -> bool {
        let Some(key) = normalize_entry_name(raw_name) else {
            log::warn!("archive: skipping unsafe entry name {raw_name:?}");
            return false;
        };
        if key == root() {
            return false;
        }
        let Some(parent) = self.ensure_dirs(key.parent().unwrap_or(Path::new("/"))) else {
            log::warn!("archive: skipping {raw_name:?}, a parent is not a directory");
            return false;
        };

        let is_dir = matches!(kind, NodeKind::Dir);
        match self.nodes.get_mut(&key) {
            // An explicit directory entry after its implicit creation: keep
            // the children, take the real metadata.
            Some(existing) if is_dir && matches!(existing.kind, NodeKind::Dir) => {
                existing.meta = meta;
                return true;
            }
            Some(existing) if matches!(existing.kind, NodeKind::Dir) => {
                if self.children.get(&key).is_some_and(|c| !c.is_empty()) {
                    log::warn!("archive: skipping {raw_name:?}, a directory has that name");
                    return false;
                }
                self.children.remove(&key);
            }
            _ => {}
        }

        if is_dir {
            self.children.entry(key.clone()).or_default();
        }
        let name = key
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.children.entry(parent).or_default().insert(name);
        self.nodes.insert(key, Node { kind, meta });
        true
    }

    /// Make every directory on the way to `dir` exist; `None` if one of them
    /// is already something other than a directory.
    fn ensure_dirs(&mut self, dir: &Path) -> Option<PathBuf> {
        let mut current = root();
        for component in dir.components().skip(1) {
            let name = component.as_os_str().to_string_lossy().into_owned();
            let next = current.join(&name);
            match self.nodes.get(&next) {
                Some(node) if !matches!(node.kind, NodeKind::Dir) => return None,
                Some(_) => {}
                None => {
                    self.nodes
                        .insert(next.clone(), dir_node(VfsMetadata::directory()));
                    self.children.insert(next.clone(), BTreeSet::new());
                    self.children
                        .entry(current.clone())
                        .or_default()
                        .insert(name);
                }
            }
            current = next;
        }
        Some(current)
    }

    /// The entry at `path` itself, links not followed.
    pub(crate) fn get(&self, path: &Path) -> Option<(PathBuf, &Node)> {
        let key = normalize_query(path)?;
        let node = self.nodes.get(&key)?;
        Some((key, node))
    }

    /// The entry at `path` with symlinks and hard links followed, as long as
    /// they stay inside the archive.
    pub(crate) fn resolve(&self, path: &Path) -> Option<(PathBuf, &Node)> {
        let (mut key, mut node) = self.get(path)?;
        for _ in 0..MAX_LINK_HOPS {
            let target = match &node.kind {
                NodeKind::Symlink(target) => symlink_target_key(&key, target)?,
                NodeKind::HardLink(target) => target.clone(),
                NodeKind::Dir | NodeKind::File(_) => return Some((key, node)),
            };
            (key, node) = self.get(&target)?;
        }
        None
    }

    /// Where a symlink at `link` pointing to `target` lands when it is
    /// extracted with `top` as the extraction root, or `None` when the
    /// kernel's resolution would leave `top` at any step.
    ///
    /// Unlike [`symlink_target_key`], which applies `..` textually, this
    /// walks the target the way the kernel does: a component that is itself
    /// a link in the archive is expanded before the next `..` is applied.
    /// So `s/up -> ..` plus `esc -> s/up/../..` is rejected, although both
    /// look harmless one at a time.
    pub(crate) fn link_target_within(
        &self,
        link: &Path,
        target: &str,
        top: &Path,
    ) -> Option<PathBuf> {
        let plain = |path: &Path| -> Vec<String> {
            path.components()
                .filter_map(|c| match c {
                    Component::Normal(p) => Some(p.to_string_lossy().into_owned()),
                    _ => None,
                })
                .collect()
        };
        let floor = plain(top).len();
        let mut stack = plain(link.parent()?);
        if stack.len() < floor || !link.starts_with(top) {
            return None;
        }
        let mut pending: std::collections::VecDeque<String> = split_link_target(target)?.collect();
        let mut hops = 0;
        while let Some(part) = pending.pop_front() {
            if part == ".." {
                if stack.len() <= floor {
                    return None;
                }
                stack.pop();
                continue;
            }
            stack.push(part);
            let key = root().join(stack.join("/"));
            if let Some(Node {
                kind: NodeKind::Symlink(next),
                ..
            }) = self.nodes.get(&key)
            {
                hops += 1;
                if hops > MAX_LINK_HOPS {
                    return None;
                }
                stack.pop();
                for part in split_link_target(next)?
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                {
                    pending.push_front(part);
                }
            }
        }
        Some(root().join(stack.join("/")))
    }

    /// Names of the direct children of the directory `key`.
    pub(crate) fn children(&self, key: &Path) -> Option<&BTreeSet<String>> {
        self.children.get(key)
    }

    /// `key` and everything below it, in path order.
    pub(crate) fn subtree<'a>(
        &'a self,
        key: &'a Path,
    ) -> impl Iterator<Item = (&'a PathBuf, &'a Node)> + 'a {
        self.nodes
            .range::<Path, _>((std::ops::Bound::Included(key), std::ops::Bound::Unbounded))
            .take_while(move |(path, _)| path.starts_with(key))
    }
}

fn root() -> PathBuf {
    PathBuf::from("/")
}

fn dir_node(meta: VfsMetadata) -> Node {
    Node {
        kind: NodeKind::Dir,
        meta,
    }
}

/// Turn a raw archive entry name into an index key, or `None` when the name
/// is unsafe. Both `/` and `\` separate components: zip tools on Windows
/// write backslashes, and a literal backslash in a name would otherwise
/// become a separator when extracting on Windows.
pub(crate) fn normalize_entry_name(raw: &str) -> Option<PathBuf> {
    let mut key = root();
    for part in raw.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => return None,
            _ => {
                // A component must stay a single plain component on this
                // platform: this rejects drive prefixes like `C:` on Windows.
                let mut parsed = Path::new(part).components();
                match (parsed.next(), parsed.next()) {
                    (Some(Component::Normal(_)), None) if !part.contains('\0') => key.push(part),
                    _ => return None,
                }
            }
        }
    }
    Some(key)
}

/// Normalise a lookup path (a `VfsPath::path` inside the archive): `.` is
/// dropped and `..` is resolved lexically, never above the root.
pub(crate) fn normalize_query(path: &Path) -> Option<PathBuf> {
    let mut parts: Vec<&std::ffi::OsStr> = Vec::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) => return None,
            Component::RootDir | Component::CurDir => {}
            Component::ParentDir => {
                parts.pop()?;
            }
            Component::Normal(part) => parts.push(part),
        }
    }
    let mut key = root();
    key.extend(parts);
    Some(key)
}

/// Key of the entry a symlink at `link` points to, or `None` for an
/// absolute target or one that climbs out of the archive root.
pub(crate) fn symlink_target_key(link: &Path, target: &str) -> Option<PathBuf> {
    if target.starts_with('/') || target.starts_with('\\') {
        return None;
    }
    let mut parts: Vec<&str> = link
        .parent()?
        .components()
        .filter_map(|c| match c {
            Component::Normal(p) => p.to_str(),
            _ => None,
        })
        .collect();
    for part in target.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            _ => parts.push(part),
        }
    }
    normalize_entry_name(&parts.join("/"))
}

/// The components of a relative link target, `.` and empty ones dropped;
/// `None` for an absolute target or a component that is not a plain name.
fn split_link_target(target: &str) -> Option<impl Iterator<Item = String> + '_> {
    if target.starts_with('/') || target.starts_with('\\') {
        return None;
    }
    let parts = target
        .split(['/', '\\'])
        .filter(|p| !p.is_empty() && *p != ".");
    if parts.clone().any(|p| {
        p != ".." && {
            let mut parsed = Path::new(p).components();
            !matches!(
                (parsed.next(), parsed.next()),
                (Some(Component::Normal(_)), None)
            ) || p.contains('\0')
        }
    }) {
        return None;
    }
    Some(parts.map(str::to_owned))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file() -> NodeKind {
        NodeKind::File(EntrySource::Tar(0))
    }

    fn names(index: &ArchiveIndex, dir: &str) -> Vec<String> {
        index
            .children(Path::new(dir))
            .unwrap()
            .iter()
            .cloned()
            .collect()
    }

    #[test]
    fn unsafe_names_never_enter_the_index() {
        assert_eq!(normalize_entry_name("../etc/passwd"), None);
        assert_eq!(normalize_entry_name("a/../../b"), None);
        assert_eq!(normalize_entry_name("a\\..\\b"), None);
        assert_eq!(normalize_entry_name("a/b\0c"), None);
        assert_eq!(
            normalize_entry_name("/abs/./path//x"),
            Some(PathBuf::from("/abs/path/x"))
        );
        assert_eq!(
            normalize_entry_name("win\\style\\name.txt"),
            Some(PathBuf::from("/win/style/name.txt"))
        );
    }

    #[cfg(windows)]
    #[test]
    fn drive_prefixes_are_rejected_on_windows() {
        assert_eq!(normalize_entry_name("C:/Windows/evil.dll"), None);
    }

    #[test]
    fn parents_are_created_implicitly_and_merged_with_explicit_entries() {
        let mut index = ArchiveIndex::new();
        assert!(index.insert("a/b/c.txt", file(), VfsMetadata::file(3)));
        assert!(index.insert(
            "a/",
            NodeKind::Dir,
            VfsMetadata::directory().with_permissions(0o700)
        ));
        assert_eq!(names(&index, "/"), ["a"]);
        assert_eq!(names(&index, "/a"), ["b"]);
        assert_eq!(names(&index, "/a/b"), ["c.txt"]);
        let (_, a) = index.get(Path::new("/a")).unwrap();
        assert_eq!(a.meta.permissions, Some(0o700));
    }

    #[test]
    fn a_later_duplicate_replaces_the_earlier_entry() {
        let mut index = ArchiveIndex::new();
        index.insert(
            "x",
            NodeKind::File(EntrySource::Tar(0)),
            VfsMetadata::file(1),
        );
        index.insert(
            "x",
            NodeKind::File(EntrySource::Tar(5)),
            VfsMetadata::file(9),
        );
        let (_, node) = index.get(Path::new("/x")).unwrap();
        assert!(matches!(node.kind, NodeKind::File(EntrySource::Tar(5))));
        assert_eq!(names(&index, "/"), ["x"]);
    }

    #[test]
    fn a_file_cannot_be_the_parent_of_another_entry() {
        let mut index = ArchiveIndex::new();
        index.insert("f", file(), VfsMetadata::file(1));
        assert!(!index.insert("f/inner", file(), VfsMetadata::file(1)));
        assert!(index.get(Path::new("/f/inner")).is_none());
    }

    #[test]
    fn links_resolve_only_inside_the_archive() {
        let mut index = ArchiveIndex::new();
        index.insert("lib/libx.so.1", file(), VfsMetadata::file(1));
        index.insert(
            "lib/libx.so",
            NodeKind::Symlink("libx.so.1".into()),
            VfsMetadata::file(0),
        );
        index.insert(
            "up",
            NodeKind::Symlink("../outside".into()),
            VfsMetadata::file(0),
        );
        index.insert(
            "abs",
            NodeKind::Symlink("/etc/passwd".into()),
            VfsMetadata::file(0),
        );
        index.insert(
            "loop",
            NodeKind::Symlink("loop".into()),
            VfsMetadata::file(0),
        );
        index.insert(
            "hard",
            NodeKind::HardLink(PathBuf::from("/lib/libx.so")),
            VfsMetadata::file(0),
        );

        let (key, _) = index.resolve(Path::new("/lib/libx.so")).unwrap();
        assert_eq!(key, PathBuf::from("/lib/libx.so.1"));
        let (key, _) = index.resolve(Path::new("/hard")).unwrap();
        assert_eq!(key, PathBuf::from("/lib/libx.so.1"));
        assert!(index.resolve(Path::new("/up")).is_none());
        assert!(index.resolve(Path::new("/abs")).is_none());
        assert!(index.resolve(Path::new("/loop")).is_none());
    }

    #[test]
    fn subtree_is_the_entry_and_its_descendants_only() {
        let mut index = ArchiveIndex::new();
        for name in ["a/1", "a/sub/2", "a b/3", "ab/4"] {
            index.insert(name, file(), VfsMetadata::file(1));
        }
        let keys: Vec<_> = index
            .subtree(Path::new("/a"))
            .map(|(k, _)| k.display().to_string())
            .collect();
        assert_eq!(keys, ["/a", "/a/1", "/a/sub", "/a/sub/2"]);
    }

    #[test]
    fn queries_resolve_dot_segments_without_leaving_the_root() {
        assert_eq!(
            normalize_query(Path::new("/a/./b/../c/")),
            Some(PathBuf::from("/a/c"))
        );
        assert_eq!(normalize_query(Path::new("/..")), None);
    }

    #[test]
    fn link_chains_are_resolved_like_the_kernel_does() {
        let mut index = ArchiveIndex::new();
        index.insert("lib/libx.so.1", file(), VfsMetadata::file(1));
        let link = |index: &mut ArchiveIndex, name: &str, target: &str| {
            index.insert(name, NodeKind::Symlink(target.into()), VfsMetadata::file(0));
        };
        link(&mut index, "s/up", "..");
        link(&mut index, "esc", "s/up/../..");
        link(&mut index, "lib/libx.so", "libx.so.1");
        link(&mut index, "usr", "lib");
        link(&mut index, "via", "usr/libx.so");
        link(&mut index, "a", "b");
        link(&mut index, "b", "a");
        link(&mut index, "loop", "a/x");
        let top = Path::new("/");
        let within =
            |name: &str, target: &str| index.link_target_within(Path::new(name), target, top);

        // Each looks harmless textually; the chain climbs above the root.
        assert_eq!(
            symlink_target_key(Path::new("/esc"), "s/up/../.."),
            Some(root())
        );
        assert_eq!(within("/s/up", ".."), Some(root()));
        assert_eq!(within("/esc", "s/up/../.."), None);

        assert_eq!(
            within("/lib/libx.so", "libx.so.1"),
            Some(PathBuf::from("/lib/libx.so.1"))
        );
        assert_eq!(
            within("/via", "usr/libx.so"),
            Some(PathBuf::from("/lib/libx.so.1"))
        );
        assert_eq!(within("/loop", "a/x"), None);
        assert_eq!(within("/abs", "/etc"), None);
        assert_eq!(
            index.link_target_within(Path::new("/s/up"), "..", Path::new("/s")),
            None
        );
    }
}

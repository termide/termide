//! Projects open in this instance.
//!
//! Switching to another project parks the one being left instead of
//! dropping its panels, so the programs in its terminals keep running and
//! its unsaved edits stay in memory. A project stays open until it is closed
//! explicitly or termide quits; the list never changes by itself.

use std::path::{Path, PathBuf};

/// What the menus show of an open project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenProjectView {
    pub root: PathBuf,
    /// A panel of this parked project waits for the user (a question to
    /// answer, finished work, a bell). Never set for the current project.
    pub attention: bool,
}

struct Entry<P> {
    root: PathBuf,
    /// The parked project; `None` for the current one, whose panels are on
    /// screen.
    parked: Option<P>,
    /// When the project was last current, on the `clock` scale.
    last_used: u64,
}

/// The open projects, in the order they were opened. Exactly one of them is
/// current; `P` is what a parked one keeps.
pub struct OpenProjects<P> {
    entries: Vec<Entry<P>>,
    clock: u64,
}

impl<P> OpenProjects<P> {
    pub fn new(current: PathBuf) -> Self {
        Self {
            entries: vec![Entry {
                root: current,
                parked: None,
                last_used: 0,
            }],
            clock: 0,
        }
    }

    fn current_index(&self) -> usize {
        self.entries
            .iter()
            .position(|entry| entry.parked.is_none())
            .expect("one open project is always current")
    }

    pub fn current(&self) -> &Path {
        &self.entries[self.current_index()].root
    }

    pub fn is_open(&self, root: &Path) -> bool {
        self.entries.iter().any(|entry| entry.root == root)
    }

    /// How many projects are open; never zero, the current one is.
    pub fn count(&self) -> usize {
        self.entries.len()
    }

    /// Make `root` current, parking the current project as `leaving`.
    ///
    /// Returns what to show for `root`: its parked state when it was open,
    /// `leaving` itself when `root` already is the current project, and
    /// `None` when `root` opens now and has to be loaded.
    pub fn switch(&mut self, root: PathBuf, leaving: P) -> Option<P> {
        let current = self.current_index();
        if self.entries[current].root == root {
            return Some(leaving);
        }
        self.clock += 1;
        self.entries[current].parked = Some(leaving);
        match self.entries.iter().position(|entry| entry.root == root) {
            Some(index) => {
                self.entries[index].last_used = self.clock;
                self.entries[index].parked.take()
            }
            None => {
                self.entries.push(Entry {
                    root,
                    parked: None,
                    last_used: self.clock,
                });
                None
            }
        }
    }

    /// Close the parked project at `root` and hand back what it kept. The
    /// current project is never closed.
    pub fn close(&mut self, root: &Path) -> Option<P> {
        let index = self
            .entries
            .iter()
            .position(|entry| entry.root == root && entry.parked.is_some())?;
        self.entries.remove(index).parked
    }

    /// The current project's root moved to `root`.
    pub fn move_current(&mut self, root: PathBuf) {
        let current = self.current_index();
        self.entries[current].root = root;
    }

    /// Every open project's root, in the order they were opened.
    pub fn roots(&self) -> impl Iterator<Item = &Path> {
        self.entries.iter().map(|entry| entry.root.as_path())
    }

    /// Every open project's root, the most recently used first: the current
    /// one, then the one left last, and so on.
    pub fn by_recent_use(&self) -> Vec<&Path> {
        let mut entries: Vec<&Entry<P>> = self.entries.iter().collect();
        entries.sort_by_key(|entry| (entry.parked.is_some(), std::cmp::Reverse(entry.last_used)));
        entries
            .into_iter()
            .map(|entry| entry.root.as_path())
            .collect()
    }

    /// The parked projects, in the order they were opened.
    pub fn parked(&self) -> impl Iterator<Item = (&Path, &P)> {
        self.entries
            .iter()
            .filter_map(|entry| Some((entry.root.as_path(), entry.parked.as_ref()?)))
    }

    pub fn parked_mut(&mut self) -> impl Iterator<Item = (&Path, &mut P)> {
        self.entries
            .iter_mut()
            .filter_map(|entry| Some((entry.root.as_path(), entry.parked.as_mut()?)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roots<P>(open: &OpenProjects<P>) -> Vec<&str> {
        open.roots().map(|p| p.to_str().unwrap()).collect()
    }

    fn recent<P>(open: &OpenProjects<P>) -> Vec<&str> {
        open.by_recent_use()
            .into_iter()
            .map(|p| p.to_str().unwrap())
            .collect()
    }

    #[test]
    fn switching_parks_the_project_left_and_restores_it_on_return() {
        let mut open = OpenProjects::new(PathBuf::from("/a"));
        assert_eq!(open.switch(PathBuf::from("/b"), "panels of a"), None);
        assert_eq!(open.current(), Path::new("/b"));
        assert_eq!(
            open.parked().collect::<Vec<_>>(),
            vec![(Path::new("/a"), &"panels of a")]
        );

        assert_eq!(
            open.switch(PathBuf::from("/a"), "panels of b"),
            Some("panels of a")
        );
        assert_eq!(open.current(), Path::new("/a"));
        assert_eq!(
            open.parked().collect::<Vec<_>>(),
            vec![(Path::new("/b"), &"panels of b")]
        );
    }

    #[test]
    fn switching_to_the_current_project_hands_its_panels_back() {
        let mut open = OpenProjects::new(PathBuf::from("/a"));
        assert_eq!(open.switch(PathBuf::from("/a"), "panels"), Some("panels"));
        assert_eq!(open.count(), 1);
        assert_eq!(open.parked().count(), 0);
    }

    #[test]
    fn opening_order_stays_while_recent_use_follows_switches() {
        let mut open = OpenProjects::new(PathBuf::from("/a"));
        open.switch(PathBuf::from("/b"), 0);
        open.switch(PathBuf::from("/c"), 0);
        open.switch(PathBuf::from("/a"), 0);
        assert_eq!(roots(&open), vec!["/a", "/b", "/c"]);
        assert_eq!(recent(&open), vec!["/a", "/c", "/b"]);
    }

    #[test]
    fn only_parked_projects_close() {
        let mut open = OpenProjects::new(PathBuf::from("/a"));
        open.switch(PathBuf::from("/b"), "panels of a");
        assert_eq!(open.close(Path::new("/b")), None, "the current one stays");
        assert_eq!(open.close(Path::new("/x")), None);
        assert_eq!(open.close(Path::new("/a")), Some("panels of a"));
        assert_eq!(roots(&open), vec!["/b"]);
    }

    #[test]
    fn moving_the_current_project_keeps_its_place() {
        let mut open = OpenProjects::new(PathBuf::from("/a"));
        open.switch(PathBuf::from("/b"), 0);
        open.move_current(PathBuf::from("/moved"));
        assert_eq!(roots(&open), vec!["/a", "/moved"]);
        assert!(open.is_open(Path::new("/moved")));
        assert!(!open.is_open(Path::new("/b")));
    }
}

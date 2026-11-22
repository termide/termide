//! Commit-graph layout engine.
//!
//! Renders a commit graph with proper box-drawing junctions (`● │ ├ ╮ ╯ ╭ ╰ ─`)
//! computed from each commit's parent hashes, instead of restyling git's
//! diagonal ASCII output. It is a **pure** fold over `(hash, parents)` so it
//! can be unit-tested in isolation; [`GraphLayout`] carries the fold from one
//! piece of history to the next, which is how [`crate::LogStream`] feeds the
//! git-log panel (gated by the `git_log.unicode_graph` setting).
//!
//! Layout model (lazygit/tig-flavoured):
//! - One **commit row** per commit: `●` in the commit's lane, `│` for every
//!   other active lane.
//! - A **connector row** is emitted *above* a commit when several lanes fold
//!   into it (a merge target / shared parent) and *below* a commit when it is a
//!   merge (extra parents branch out to new lanes).
//! - Lanes are one column wide and contiguous, so corners connect cleanly
//!   (`├╮`, `├╯`). Where a horizontal run crosses an unrelated active lane or
//!   passes through an intermediate junction, the through-glyphs `┼`/`┴`/`┬`
//!   are used so the line stays unbroken.
//! - Freed lanes are reused for later branches (`free_slot` fills holes) but
//!   never shift-compacted: pulling a lane leftward would need a 1-cell diagonal
//!   transition, which box-drawing cannot render flush against `│`. The cost is
//!   an occasional gap column or a longer horizontal `─` run — by design.
//!
//! Known prototype gaps: no shift-compaction (above), and octopus merges
//! (3+ parents) draw correctly but aren't width-optimised.

/// A commit and its parent hashes, in the order `git log` returns them
/// (newest first; first parent is the mainline).
#[derive(Debug, Clone)]
pub struct GraphCommit {
    /// Commit hash (any stable id).
    pub hash: String,
    /// Parent hashes; `parents[0]` is the first (mainline) parent.
    pub parents: Vec<String>,
}

/// One rendered row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphRow {
    /// Box-drawing cells for this row.
    pub graph: String,
    /// `Some(i)` when this row belongs to `commits[i]`; `None` for a connector.
    pub commit: Option<usize>,
}

/// Index of the first free (`None`) lane, allocating a new one if needed.
fn free_slot(lanes: &mut Vec<Option<String>>) -> usize {
    if let Some(i) = lanes.iter().position(Option::is_none) {
        i
    } else {
        lanes.push(None);
        lanes.len() - 1
    }
}

fn render(cells: &[char]) -> String {
    cells.iter().collect::<String>().trim_end().to_string()
}

/// Lay out and render the commit graph. Returns rows top-to-bottom (commit rows
/// interleaved with connector rows).
pub fn render_graph(commits: &[GraphCommit]) -> Vec<GraphRow> {
    let mut layout = GraphLayout::default();
    let mut rows = Vec::new();
    for (idx, c) in commits.iter().enumerate() {
        layout.push(c, idx, &mut rows);
    }
    rows
}

/// The layout carried from one commit to the next, so a history read in
/// pieces renders exactly as it would in one go: the lanes are all the state
/// the graph has.
#[derive(Debug, Clone, Default)]
pub struct GraphLayout {
    /// Each active lane holds the hash it is currently routing toward.
    lanes: Vec<Option<String>>,
}

impl GraphLayout {
    /// Lay out the next commit `c` (newer ones already pushed), appending its
    /// rows to `rows`; its commit row refers to `commits[idx]`.
    pub fn push(&mut self, c: &GraphCommit, idx: usize, rows: &mut Vec<GraphRow>) {
        let lanes = &mut self.lanes;
        // Lanes whose pending hash is this commit (its children's edges).
        let incoming: Vec<usize> = lanes
            .iter()
            .enumerate()
            .filter(|(_, l)| l.as_deref() == Some(c.hash.as_str()))
            .map(|(i, _)| i)
            .collect();

        let commit_col = match incoming.first() {
            Some(&i) => i,
            None => free_slot(lanes), // a tip not referenced by any child
        };

        // --- connector ABOVE: fold extra incoming lanes into commit_col ---
        // `commit_col` is the leftmost incoming lane, so all folds come from the
        // right and the run spans `commit_col..=hi`.
        if incoming.len() > 1 {
            let width = lanes.len();
            let mut cells = vec!['│'; width];
            for (i, l) in lanes.iter().enumerate() {
                if l.is_none() {
                    cells[i] = ' ';
                }
            }
            let hi = *incoming.iter().max().unwrap();
            // horizontal run: cross unrelated active lanes with `┼`, fill gaps `─`
            for cell in &mut cells[commit_col..=hi] {
                *cell = if *cell == '│' { '┼' } else { '─' };
            }
            for &i in &incoming {
                cells[i] = if i == commit_col {
                    '├' // mainline continues down + accepts the fold from the right
                } else if i == hi {
                    '╯' // furthest fold turns up-and-left
                } else {
                    '┴' // intermediate fold: up-and-left while the run passes through
                };
            }
            rows.push(GraphRow {
                graph: render(&cells),
                commit: None,
            });
            for &i in incoming.iter().skip(1) {
                lanes[i] = None;
            }
        }

        lanes[commit_col] = Some(c.hash.clone());

        // --- commit row ---
        let width = lanes.len();
        let mut cells = vec![' '; width];
        for (i, l) in lanes.iter().enumerate() {
            cells[i] = if i == commit_col {
                '●'
            } else if l.is_some() {
                '│'
            } else {
                ' '
            };
        }
        rows.push(GraphRow {
            graph: render(&cells),
            commit: Some(idx),
        });

        // --- advance lanes to this commit's parents ---
        match c.parents.first() {
            Some(p0) => lanes[commit_col] = Some(p0.clone()),
            None => lanes[commit_col] = None, // root: lane ends
        }
        let mut branched: Vec<usize> = Vec::new();
        for p in c.parents.iter().skip(1) {
            let col = match lanes.iter().position(|l| l.as_deref() == Some(p.as_str())) {
                Some(i) => i, // shares a lane already heading to that parent
                None => {
                    let i = free_slot(lanes);
                    lanes[i] = Some(p.clone());
                    i
                }
            };
            branched.push(col);
        }

        // --- connector BELOW: branch the merge parents out of commit_col ---
        // Reused lanes (`free_slot` fills holes) can sit on either side of
        // `commit_col`, so the run spans `lo..=hi` and curves accordingly.
        if !branched.is_empty() {
            let width = lanes.len();
            let mut cells = vec!['│'; width];
            for (i, l) in lanes.iter().enumerate() {
                if l.is_none() {
                    cells[i] = ' ';
                }
            }
            let lo = *branched.iter().min().unwrap().min(&commit_col);
            let hi = *branched.iter().max().unwrap().max(&commit_col);
            for cell in &mut cells[lo..=hi] {
                *cell = if *cell == '│' { '┼' } else { '─' };
            }
            let has_left = branched.iter().any(|&c| c < commit_col);
            let has_right = branched.iter().any(|&c| c > commit_col);
            cells[commit_col] = match (has_left, has_right) {
                (true, true) => '┼',
                (true, false) => '┤',
                _ => '├', // mainline down + branches right
            };
            for &col in &branched {
                if col == commit_col {
                    continue;
                }
                cells[col] = if col > commit_col {
                    if col == hi {
                        '╮'
                    } else {
                        '┬'
                    } // down-from-left
                } else if col == lo {
                    '╭' // down-from-right
                } else {
                    '┬'
                };
            }
            rows.push(GraphRow {
                graph: render(&cells),
                commit: None,
            });
        }
    }
}

/// Parse `git log --format=%h\t%p` (tab between hash and the space-separated
/// parent list) into [`GraphCommit`]s, ready for [`render_graph`].
pub fn parse_parents_log(stdout: &str) -> Vec<GraphCommit> {
    stdout
        .lines()
        .filter_map(|line| {
            let (hash, parents) = line.split_once('\t')?;
            if hash.is_empty() {
                return None;
            }
            Some(GraphCommit {
                hash: hash.to_string(),
                parents: parents.split_whitespace().map(str::to_string).collect(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(hash: &str, parents: &[&str]) -> GraphCommit {
        GraphCommit {
            hash: hash.to_string(),
            parents: parents.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// Pretty-print rows next to their commit hash for eyeballing.
    fn dump(commits: &[GraphCommit], rows: &[GraphRow]) -> String {
        rows.iter()
            .map(|r| match r.commit {
                Some(i) => format!("{:<6}{}", r.graph, commits[i].hash),
                None => r.graph.clone(),
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn linear_history_is_a_single_lane() {
        let commits = [c("a", &["b"]), c("b", &["d"]), c("d", &[])];
        let rows = render_graph(&commits);
        // No connectors; one `●` row per commit.
        assert_eq!(rows.iter().filter(|r| r.commit.is_some()).count(), 3);
        assert!(rows.iter().all(|r| r.graph == "●" || r.graph.is_empty()));
    }

    #[test]
    fn branch_then_merge_uses_corners() {
        // a (merge of b and side s) ; s and b both lead to base d.
        //   a  ── merge(b, s)
        //   |\
        //   | s
        //   |/
        //   b
        //   d
        let commits = [
            c("a", &["b", "s"]),
            c("s", &["b"]),
            c("b", &["d"]),
            c("d", &[]),
        ];
        let rows = render_graph(&commits);
        let out = dump(&commits, &rows);
        println!("\n{out}\n");

        let graphs: Vec<&str> = rows.iter().map(|r| r.graph.as_str()).collect();
        // merge commit branches a second lane out (├╮ connector below it)
        assert!(
            graphs.contains(&"├╮"),
            "expected a ├╮ branch row, got {graphs:?}"
        );
        // the side lane folds back in (├╯ connector above the merge base)
        assert!(
            graphs.contains(&"├╯"),
            "expected a ├╯ fold row, got {graphs:?}"
        );
        // every commit got exactly one row
        assert_eq!(rows.iter().filter(|r| r.commit.is_some()).count(), 4);
    }

    #[test]
    fn three_way_fold_uses_through_glyph() {
        // Three tips that all share parent `g` fold in one row: the middle lane
        // must use `┴` (up-left + run passes through), not a second `╯`.
        let commits = [c("a", &["g"]), c("b", &["g"]), c("d", &["g"]), c("g", &[])];
        let rows = render_graph(&commits);
        let out = dump(&commits, &rows);
        println!("\n{out}\n");
        assert!(
            rows.iter().any(|r| r.graph == "├┴╯"),
            "expected a ├┴╯ fold row, got {:?}",
            rows.iter().map(|r| r.graph.as_str()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn horizontal_run_crosses_unrelated_lane_with_plus() {
        // An unrelated lane (`y`→base) stays active while two `p` lanes fold
        // across it; the crossing cell must be `┼`, not a broken `│`.
        let commits = [
            c("x", &["p"]),
            c("y", &["base"]),
            c("z", &["p"]),
            c("p", &["base"]),
            c("base", &[]),
        ];
        let rows = render_graph(&commits);
        let out = dump(&commits, &rows);
        println!("\n{out}\n");
        assert!(
            rows.iter().any(|r| r.graph.contains('┼')),
            "expected a ┼ crossing, got {:?}",
            rows.iter().map(|r| r.graph.as_str()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn two_stacked_merges_render() {
        // Mirrors the user's history: two merge commits, each pulling a
        // one-commit side branch back into the mainline.
        let commits = [
            c("m1", &["m2", "x"]),
            c("x", &["m2"]),
            c("m2", &["base", "y"]),
            c("y", &["base"]),
            c("base", &[]),
        ];
        let rows = render_graph(&commits);
        println!("\n{}\n", dump(&commits, &rows));
        // Two merges → at least two branch rows and two fold rows.
        let branches = rows.iter().filter(|r| r.graph.contains('╮')).count();
        let folds = rows.iter().filter(|r| r.graph.contains('╯')).count();
        assert!(
            branches >= 2 && folds >= 2,
            "branches={branches} folds={folds}"
        );
        assert_eq!(rows.iter().filter(|r| r.commit.is_some()).count(), 5);
    }
}

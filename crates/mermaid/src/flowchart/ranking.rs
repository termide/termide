//! Node ranking: longest-path layer assignment with DFS cycle breaking.

use std::collections::HashSet;

use crate::parser::Flowchart;

/// Longest-path rank assignment, ignoring back edges (cycle support).
pub(super) fn assign_ranks(fc: &Flowchart) -> Vec<usize> {
    let n = fc.nodes.len();
    let mut succ: Vec<Vec<usize>> = vec![Vec::new(); n];
    for e in &fc.edges {
        if e.from != e.to {
            succ[e.from].push(e.to);
        }
    }

    let mut visited = vec![false; n];
    let mut onstack = vec![false; n];
    let mut order = Vec::new();
    let mut back: HashSet<(usize, usize)> = HashSet::new();
    for u in 0..n {
        if !visited[u] {
            dfs(u, &succ, &mut visited, &mut onstack, &mut order, &mut back);
        }
    }
    order.reverse(); // reverse postorder ≈ topological order

    let mut rank = vec![0usize; n];
    for &u in &order {
        for &v in &succ[u] {
            if !back.contains(&(u, v)) {
                rank[v] = rank[v].max(rank[u] + 1);
            }
        }
    }
    rank
}

fn dfs(
    u: usize,
    succ: &[Vec<usize>],
    visited: &mut [bool],
    onstack: &mut [bool],
    order: &mut Vec<usize>,
    back: &mut HashSet<(usize, usize)>,
) {
    visited[u] = true;
    onstack[u] = true;
    for &v in &succ[u] {
        if onstack[v] {
            back.insert((u, v));
        } else if !visited[v] {
            dfs(v, succ, visited, onstack, order, back);
        }
    }
    onstack[u] = false;
    order.push(u);
}

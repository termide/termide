//! `gitGraph` parsing: commit/branch/checkout/switch/merge operations.

#[derive(Debug, Clone)]
pub enum GitOp {
    Commit { label: String },
    Branch(String),
    Checkout(String),
    Merge(String),
}

#[derive(Debug, Clone, Default)]
pub struct GitGraph {
    pub ops: Vec<GitOp>,
}

/// Parse a `gitGraph`: `commit`/`branch`/`checkout`/`switch`/`merge`.
pub fn parse_gitgraph(src: &str) -> GitGraph {
    let mut g = GitGraph::default();
    for raw in src.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("%%") || line.starts_with("gitGraph") {
            continue;
        }
        let kw = line.split_whitespace().next().unwrap_or("");
        match kw {
            "commit" => {
                // Prefer an explicit `id:` then `tag:`, else empty.
                let label = extract_quoted(line, "id:")
                    .or_else(|| extract_quoted(line, "tag:"))
                    .unwrap_or_default();
                g.ops.push(GitOp::Commit { label });
            }
            "branch" => {
                if let Some(name) = line.split_whitespace().nth(1) {
                    g.ops.push(GitOp::Branch(name.to_string()));
                }
            }
            "checkout" | "switch" => {
                if let Some(name) = line.split_whitespace().nth(1) {
                    g.ops.push(GitOp::Checkout(name.to_string()));
                }
            }
            "merge" => {
                if let Some(name) = line.split_whitespace().nth(1) {
                    g.ops.push(GitOp::Merge(name.to_string()));
                }
            }
            _ => {}
        }
    }
    g
}

/// Extract the quoted value following `key` (e.g. `id: "A"` → `A`).
fn extract_quoted(line: &str, key: &str) -> Option<String> {
    let after = &line[line.find(key)? + key.len()..];
    let start = after.find('"')? + 1;
    let end = after[start..].find('"')? + start;
    Some(after[start..end].to_string())
}

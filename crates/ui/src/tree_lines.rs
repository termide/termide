//! The lines a tree is drawn with (`├─ `, `│  `, `└─ `), from the depths of
//! its visible rows.

/// The prefix of each row of a tree whose visible rows have `depths`, in
/// order: depth 0 is a root and gets none, a deeper row gets `├─ ` or `└─ `
/// at its own level and `│  ` or spaces at the levels above. O(n).
pub fn tree_prefixes(depths: &[usize]) -> Vec<String> {
    let max_depth = depths.iter().copied().max().unwrap_or(0);
    // has_next[lvl]: a later row sits at that depth before a shallower one
    // cuts it off.
    let mut has_next = vec![false; max_depth + 1];
    let mut prefixes = Vec::with_capacity(depths.len());
    for &depth in depths.iter().rev() {
        if depth == 0 {
            has_next.fill(false);
            has_next[0] = true;
            prefixes.push(String::new());
            continue;
        }
        let mut prefix = String::with_capacity(depth * 3);
        for (lvl, next) in has_next[1..=depth].iter().enumerate() {
            prefix.push_str(match (lvl + 1 == depth, *next) {
                (true, true) => "├─ ",
                (true, false) => "└─ ",
                (false, true) => "│  ",
                (false, false) => "   ",
            });
        }
        prefixes.push(prefix);
        for next in &mut has_next[(depth + 1)..=max_depth] {
            *next = false;
        }
        has_next[depth] = true;
    }
    prefixes.reverse();
    prefixes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn siblings_and_nested_rows_get_their_lines() {
        // root, a/, a/x, a/y, b
        let prefixes = tree_prefixes(&[0, 1, 2, 2, 1]);
        assert_eq!(prefixes, ["", "├─ ", "│  ├─ ", "│  └─ ", "└─ "]);
    }

    #[test]
    fn a_new_root_ends_the_lines_of_the_last() {
        let prefixes = tree_prefixes(&[0, 1, 0, 1]);
        assert_eq!(prefixes, ["", "└─ ", "", "└─ "]);
    }
}

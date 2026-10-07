//! The open projects as buttons in the menu bar, between the menu titles and
//! the indicators.
//!
//! Every button is a bare `name`, kept apart by a dim `" | "` that belongs
//! to no button of its own. The strip starts two columns after the menu
//! titles, so it reads as a block of its own. When the
//! names do not fit, they lose their start, and the room is shared fairly:
//! a short name keeps what it needs and the long ones split what is left.
//! When even cut names do not fit, buttons drop out — the one the user acts
//! on, the current project and the ones that wait stay longest — and `+N`
//! counts the hidden ones.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// An open project as the menu bar shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectButton {
    /// What the button reads: the project's directory name, with its parent
    /// when another open project has the same name.
    pub name: String,
    /// The project on screen.
    pub current: bool,
    /// A panel of this project, open in the background, waits for the user.
    pub attention: bool,
    /// Selected in the open menu or held by the mouse: the user acts on it,
    /// so it never drops out while another can.
    pub selected: bool,
}

/// A button placed in the bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacedButton {
    /// Which of the buttons given this is.
    pub index: usize,
    /// First column of the button.
    pub x: u16,
    /// The text drawn: the name, with its attention mark when it waits.
    pub label: String,
}

impl PlacedButton {
    /// Columns the button covers.
    pub fn range(&self) -> std::ops::Range<u16> {
        self.x..self.x + self.label.width() as u16
    }
}

/// The buttons as they fit in the bar.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectStrip {
    /// The buttons shown, in list order.
    pub buttons: Vec<PlacedButton>,
    /// `+N` for the buttons that did not fit, and its first column.
    pub overflow: Option<(u16, String)>,
}

/// A cut name keeps at least this many columns: `…` and one character.
const MIN_NAME_WIDTH: usize = 2;

/// What follows the name of a project that waits.
fn mark(attention: bool) -> String {
    if attention {
        format!(" {}", termide_core::attention_mark())
    } else {
        String::new()
    }
}

/// `name` cut from its start to at most `width` columns, `…` in front.
fn cut_start(name: &str, width: usize) -> String {
    if name.width() <= width {
        return name.to_string();
    }
    let mut tail: Vec<char> = Vec::new();
    let mut used = 1; // the ellipsis
    for ch in name.chars().rev() {
        let w = ch.width().unwrap_or(0);
        if used + w > width {
            break;
        }
        used += w;
        tail.push(ch);
    }
    std::iter::once('…').chain(tail.into_iter().rev()).collect()
}

/// What keeps two buttons apart: a dim separator, not part of either.
pub const SEPARATOR: &str = " | ";

/// Name widths for `buttons` (indices into `all`) to fit `avail` columns,
/// or `None` when they do not fit even cut to the minimum.
fn share(all: &[ProjectButton], buttons: &[usize], avail: usize) -> Option<Vec<usize>> {
    let needs: Vec<usize> = buttons.iter().map(|&i| all[i].name.width()).collect();
    let fixed: usize = buttons
        .iter()
        .map(|&i| mark(all[i].attention).width())
        .sum::<usize>()
        + buttons.len().saturating_sub(1) * SEPARATOR.width();
    let budget = avail.checked_sub(fixed)?;
    if needs.iter().sum::<usize>() <= budget {
        return Some(needs);
    }
    let floor: usize = needs.iter().map(|&n| n.min(MIN_NAME_WIDTH)).sum();
    if floor > budget {
        return None;
    }
    // The widest cap every name can share: names shorter than it keep their
    // width, the rest are cut to it.
    let used = |cap: usize| needs.iter().map(|&n| n.min(cap)).sum::<usize>();
    let mut cap = MIN_NAME_WIDTH;
    while used(cap + 1) <= budget {
        cap += 1;
    }
    let mut spare = budget - used(cap);
    Some(
        needs
            .iter()
            .map(|&n| {
                if n > cap && spare > 0 {
                    spare -= 1;
                    cap + 1
                } else {
                    n.min(cap)
                }
            })
            .collect(),
    )
}

/// Lay `buttons` out in `width` columns from column `start`. Nothing is
/// shown for fewer than two projects: one button would only repeat what
/// the screen shows.
pub fn fit_project_strip(buttons: &[ProjectButton], start: u16, width: usize) -> ProjectStrip {
    if buttons.len() < 2 {
        return ProjectStrip::default();
    }
    let rank = |i: usize| {
        let button = &buttons[i];
        if button.selected {
            0
        } else if button.current {
            1
        } else if button.attention {
            2
        } else {
            3
        }
    };
    let mut kept: Vec<usize> = (0..buttons.len()).collect();
    loop {
        let hidden = buttons.len() - kept.len();
        let overflow = (hidden > 0).then(|| format!("+{hidden}"));
        let overflow_width = overflow.as_ref().map_or(0, |text| 1 + text.width());
        let fitted = width
            .checked_sub(overflow_width)
            .and_then(|avail| share(buttons, &kept, avail));
        if let Some(names) = fitted {
            let mut x = start;
            let mut placed = Vec::with_capacity(kept.len());
            for (step, (&index, name_width)) in kept.iter().zip(names).enumerate() {
                let button = &buttons[index];
                // The separator stands between two buttons: it belongs to
                // neither, so neither its columns nor its colour are theirs.
                if step > 0 {
                    x += SEPARATOR.width() as u16;
                }
                let label = format!(
                    "{}{}",
                    cut_start(&button.name, name_width),
                    mark(button.attention)
                );
                let label_width = label.width() as u16;
                placed.push(PlacedButton { index, x, label });
                x += label_width;
            }
            let overflow_x = if overflow.is_some() { x + 1 } else { x };
            return ProjectStrip {
                buttons: placed,
                overflow: overflow.map(|text| (overflow_x, text)),
            };
        }
        if kept.len() == 1 {
            return ProjectStrip::default();
        }
        // Drop the least needed: others before the ones that wait, then the
        // current project, the selected one last; the later in the list
        // first.
        let drop = (0..kept.len())
            .max_by_key(|&k| (rank(kept[k]), kept[k]))
            .expect("kept is never empty");
        kept.remove(drop);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn button(name: &str) -> ProjectButton {
        ProjectButton {
            name: name.to_string(),
            current: false,
            attention: false,
            selected: false,
        }
    }

    fn labels(strip: &ProjectStrip) -> Vec<&str> {
        strip.buttons.iter().map(|b| b.label.as_str()).collect()
    }

    #[test]
    fn a_single_project_shows_no_buttons() {
        assert_eq!(
            fit_project_strip(&[button("termide")], 0, 80),
            ProjectStrip::default()
        );
    }

    #[test]
    fn buttons_that_fit_stand_separator_apart_from_the_start() {
        let strip = fit_project_strip(&[button("one"), button("two")], 10, 80);
        assert_eq!(labels(&strip), ["one", "two"]);
        assert_eq!(strip.buttons[0].x, 10);
        assert_eq!(strip.buttons[1].x, 16);
        assert_eq!(strip.overflow, None);
    }

    #[test]
    fn long_names_lose_their_start_and_short_ones_stay_whole() {
        // 3 buttons: 2 separators of 3, 19 left for names: "web" keeps 3,
        // the long ones share 16.
        let strip = fit_project_strip(
            &[
                button("termide-agent"),
                button("web"),
                button("termide-links"),
            ],
            0,
            25,
        );
        assert_eq!(labels(&strip), ["…e-agent", "web", "…e-links"]);
        let used: usize =
            strip.buttons.iter().map(|b| b.label.width()).sum::<usize>() + 2 * SEPARATOR.width();
        assert_eq!(used, 25);
    }

    #[test]
    fn spare_columns_go_to_the_first_cut_names() {
        let strip = fit_project_strip(&[button("abcdef"), button("ghijkl")], 0, 12);
        // 3 for the separator, 9 for names: 5 + 4.
        assert_eq!(labels(&strip), ["…cdef", "…jkl"]);
    }

    #[test]
    fn the_bell_stays_whole_when_the_name_is_cut() {
        let mut waiting = button("termide-agent");
        waiting.attention = true;
        let strip = fit_project_strip(&[waiting, button("other")], 0, 20);
        let mark = mark(true);
        assert!(strip.buttons[0].label.ends_with(&mark));
        assert!(strip.buttons[0].label.starts_with('…'));
    }

    #[test]
    fn without_room_the_others_drop_out_before_the_current_and_waiting_ones() {
        let mut current = button("cur");
        current.current = true;
        let mut waiting = button("wait");
        waiting.attention = true;
        let buttons = [button("aaaa"), current, button("bbbb"), waiting];
        // Room for two short buttons and "+2", not for three and "+1".
        let strip = fit_project_strip(&buttons, 0, 16);
        let shown: Vec<usize> = strip.buttons.iter().map(|b| b.index).collect();
        assert_eq!(shown, [1, 3]);
        assert_eq!(strip.overflow.as_ref().map(|(_, t)| t.as_str()), Some("+2"));
    }

    #[test]
    fn the_selected_button_outlasts_the_current_one() {
        let mut current = button("cur");
        current.current = true;
        let mut selected = button("sel");
        selected.selected = true;
        let buttons = [current, button("aaaa"), selected];
        // Room for one short button and "+2" only.
        let strip = fit_project_strip(&buttons, 0, 8);
        let shown: Vec<usize> = strip.buttons.iter().map(|b| b.index).collect();
        assert_eq!(shown, [2]);
    }

    #[test]
    fn no_room_even_for_the_current_project_shows_nothing() {
        let mut current = button("cur");
        current.current = true;
        // "+1" leaves no column for a name cut to the minimum.
        let strip = fit_project_strip(&[current, button("other")], 0, 3);
        assert_eq!(strip, ProjectStrip::default());
    }

    #[test]
    fn the_separator_belongs_to_no_button() {
        let strip = fit_project_strip(&[button("one"), button("two"), button("three")], 0, 80);
        assert_eq!(strip.buttons[0].x, 0);
        assert_eq!(strip.buttons[1].x, SEPARATOR.width() as u16 + 3);
        assert_eq!(strip.buttons[2].x, (SEPARATOR.width() * 2 + 6) as u16);
        // Nothing of a button covers the separator between it and the next.
        for pair in strip.buttons.windows(2) {
            assert_eq!(pair[0].range().end + SEPARATOR.width() as u16, pair[1].x);
        }
    }
}

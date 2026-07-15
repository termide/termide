//! `sequenceDiagram` parsing: participants, arrow tokens, messages, and notes.

/// A sequence-diagram participant (in left-to-right declaration/use order).
#[derive(Debug, Clone)]
pub struct Participant {
    pub id: String,
    pub label: String,
    /// `actor` renders with a stick-figure marker; `participant` is a box.
    pub actor: bool,
}

/// Arrow line style + head, derived from the Mermaid arrow token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Arrow {
    pub dashed: bool,
    /// `>` filled head, `x` cross, `o`/`)` open — collapsed to a head glyph.
    pub head: ArrowHead,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArrowHead {
    Filled,
    Open,
    Cross,
    None,
}

/// Where a note sits relative to its participant(s).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotePlacement {
    LeftOf,
    RightOf,
    Over,
}

/// One step of a sequence diagram.
#[derive(Debug, Clone)]
pub enum SeqEvent {
    Message {
        from: usize,
        to: usize,
        text: String,
        arrow: Arrow,
    },
    Note {
        placement: NotePlacement,
        /// Indices into `participants` (one for left/right, one or two for over).
        targets: Vec<usize>,
        text: String,
    },
}

/// A parsed sequence diagram.
#[derive(Debug, Clone, Default)]
pub struct Sequence {
    pub participants: Vec<Participant>,
    pub events: Vec<SeqEvent>,
}

impl Sequence {
    fn participant_index(&mut self, id: &str) -> usize {
        if let Some(i) = self.participants.iter().position(|p| p.id == id) {
            return i;
        }
        self.participants.push(Participant {
            id: id.to_string(),
            label: id.to_string(),
            actor: false,
        });
        self.participants.len() - 1
    }
}

/// Known sequence arrow tokens, longest first so `-->>` wins over `->>`.
const ARROWS: &[(&str, Arrow)] = &[
    (
        "-->>",
        Arrow {
            dashed: true,
            head: ArrowHead::Filled,
        },
    ),
    (
        "->>",
        Arrow {
            dashed: false,
            head: ArrowHead::Filled,
        },
    ),
    (
        "--x",
        Arrow {
            dashed: true,
            head: ArrowHead::Cross,
        },
    ),
    (
        "-x",
        Arrow {
            dashed: false,
            head: ArrowHead::Cross,
        },
    ),
    (
        "--)",
        Arrow {
            dashed: true,
            head: ArrowHead::Open,
        },
    ),
    (
        "-)",
        Arrow {
            dashed: false,
            head: ArrowHead::Open,
        },
    ),
    (
        "-->",
        Arrow {
            dashed: true,
            head: ArrowHead::None,
        },
    ),
    (
        "->",
        Arrow {
            dashed: false,
            head: ArrowHead::None,
        },
    ),
];

/// Parse a `sequenceDiagram` source into a [`Sequence`]. Unrecognized lines
/// (control blocks, activations, etc.) are skipped, so partial input still
/// renders something useful.
pub fn parse_sequence(src: &str) -> Sequence {
    let mut seq = Sequence::default();

    for raw in src.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("%%") || line == "sequenceDiagram" {
            continue;
        }

        // Declarations: `participant A as Alice` / `actor Bob`.
        if let Some(rest) = line
            .strip_prefix("participant ")
            .or_else(|| line.strip_prefix("actor "))
        {
            let actor = line.starts_with("actor ");
            let (id, label) = match rest.split_once(" as ") {
                Some((id, label)) => (id.trim().to_string(), label.trim().to_string()),
                None => (rest.trim().to_string(), rest.trim().to_string()),
            };
            let idx = seq.participant_index(&id);
            seq.participants[idx].label = label;
            seq.participants[idx].actor = actor;
            continue;
        }

        // Notes: `Note left of A: text`, `Note over A,B: text`.
        if let Some(rest) = line
            .strip_prefix("Note ")
            .or_else(|| line.strip_prefix("note "))
        {
            if let Some(ev) = parse_note(&mut seq, rest) {
                seq.events.push(ev);
            }
            continue;
        }

        // Messages: `A->>B: text`.
        if let Some(ev) = parse_message(&mut seq, line) {
            seq.events.push(ev);
        }
    }

    seq
}

fn parse_note(seq: &mut Sequence, rest: &str) -> Option<SeqEvent> {
    let (placement, after) = if let Some(a) = rest.strip_prefix("left of ") {
        (NotePlacement::LeftOf, a)
    } else if let Some(a) = rest.strip_prefix("right of ") {
        (NotePlacement::RightOf, a)
    } else if let Some(a) = rest.strip_prefix("over ") {
        (NotePlacement::Over, a)
    } else {
        return None;
    };
    let (names, text) = after.split_once(':')?;
    let targets: Vec<usize> = names
        .split(',')
        .map(|n| seq.participant_index(n.trim()))
        .collect();
    if targets.is_empty() {
        return None;
    }
    Some(SeqEvent::Note {
        placement,
        targets,
        text: text.trim().to_string(),
    })
}

fn parse_message(seq: &mut Sequence, line: &str) -> Option<SeqEvent> {
    // Find the first arrow token and split the line around it.
    let (tok, arrow, at) = ARROWS
        .iter()
        .filter_map(|(tok, arrow)| line.find(tok).map(|at| (*tok, *arrow, at)))
        .min_by_key(|&(_, _, at)| at)?;

    let left = line[..at].trim();
    let after = &line[at + tok.len()..];
    let (right_raw, text) = match after.split_once(':') {
        Some((r, t)) => (r.trim(), t.trim().to_string()),
        None => (after.trim(), String::new()),
    };
    if left.is_empty() || right_raw.is_empty() {
        return None;
    }
    // Strip activation markers (`+`/`-`) that may prefix the target.
    let right = right_raw.trim_start_matches(['+', '-']).trim();
    let from = seq.participant_index(left);
    let to = seq.participant_index(right);
    Some(SeqEvent::Message {
        from,
        to,
        text,
        arrow,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_participants_and_messages() {
        let seq = parse_sequence(
            "sequenceDiagram\nparticipant A as Alice\nA->>B: Hello\nB-->>A: Hi back",
        );
        assert_eq!(seq.participants.len(), 2);
        assert_eq!(seq.participants[0].label, "Alice");
        assert_eq!(seq.participants[1].id, "B");
        assert_eq!(seq.events.len(), 2);
        match &seq.events[0] {
            SeqEvent::Message {
                from,
                to,
                text,
                arrow,
            } => {
                assert_eq!((*from, *to), (0, 1));
                assert_eq!(text, "Hello");
                assert_eq!(arrow.head, ArrowHead::Filled);
                assert!(!arrow.dashed);
            }
            _ => panic!("expected message"),
        }
        match &seq.events[1] {
            SeqEvent::Message { arrow, .. } => assert!(arrow.dashed),
            _ => panic!("expected message"),
        }
    }

    #[test]
    fn auto_registers_participants_in_use_order() {
        let seq = parse_sequence("sequenceDiagram\nBob->>Carol: x\nAlice->>Bob: y");
        let ids: Vec<&str> = seq.participants.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, vec!["Bob", "Carol", "Alice"]);
    }

    #[test]
    fn parses_notes() {
        let seq = parse_sequence("sequenceDiagram\nA->>B: hi\nNote over A,B: shared");
        let note = seq.events.iter().find_map(|e| match e {
            SeqEvent::Note {
                placement,
                targets,
                text,
            } => Some((*placement, targets.clone(), text.clone())),
            _ => None,
        });
        let (placement, targets, text) = note.expect("note parsed");
        assert_eq!(placement, NotePlacement::Over);
        assert_eq!(targets.len(), 2);
        assert_eq!(text, "shared");
    }
}

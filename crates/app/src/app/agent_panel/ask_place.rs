//! `subagent_run = "ask"`: the user picks where delegated work runs.
//!
//! An agent often hands several tasks over at once, each on a call of its
//! own. One card for them all, not one per task: the first call waits a
//! moment for the others, then asks once, listing every task — hidden, in
//! panels, or one by one, each task then asking for itself. A call with no
//! user to ask (a headless run) runs its task hidden.

use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

use termide_agent_core::{
    CancelToken, Question, QuestionOption, QuestionReply, SubagentPlace, UserAsker,
};

/// How long the first call waits for the tasks handed over with it.
const GATHER: Duration = Duration::from_millis(150);

/// The user's pick for a batch of tasks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Choice {
    Hidden,
    Panel,
    OneByOne,
}

/// The tasks one card asks about, and the answer once it is in.
struct Batch {
    tasks: Mutex<Vec<String>>,
    answer: Mutex<Option<Choice>>,
    ready: Condvar,
}

impl Batch {
    fn new(task: String) -> Self {
        Self {
            tasks: Mutex::new(vec![task]),
            answer: Mutex::new(None),
            ready: Condvar::new(),
        }
    }

    fn answer(&self, choice: Choice) {
        *self.answer.lock().unwrap_or_else(PoisonError::into_inner) = Some(choice);
        self.ready.notify_all();
    }

    /// Wait for the answer; `None` once `cancel` stops the call first.
    fn wait(&self, cancel: &CancelToken) -> Option<Choice> {
        let mut answer = self.answer.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if let Some(choice) = *answer {
                return Some(choice);
            }
            if cancel.is_cancelled() {
                return None;
            }
            answer = self
                .ready
                .wait_timeout(answer, Duration::from_millis(100))
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

/// The card being gathered for the tasks of one panel's agent.
#[derive(Default)]
pub(super) struct PlaceAsks {
    gathering: Mutex<Option<Arc<Batch>>>,
}

impl PlaceAsks {
    /// Where the task `prompt` for `agent` runs, as the user answers through
    /// `asker`: hidden or in a panel (opened when none is free).
    pub(super) fn place(
        &self,
        agent: &str,
        prompt: &str,
        asker: Option<&UserAsker>,
        cancel: &CancelToken,
    ) -> SubagentPlace {
        let Some(asker) = asker else {
            return SubagentPlace::Hidden;
        };
        let task = task_line(agent, prompt);
        let (batch, leads) = {
            let mut gathering = self
                .gathering
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            match gathering.as_ref() {
                Some(batch) => {
                    batch
                        .tasks
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push(task.clone());
                    (Arc::clone(batch), false)
                }
                None => {
                    let batch = Arc::new(Batch::new(task.clone()));
                    *gathering = Some(Arc::clone(&batch));
                    (batch, true)
                }
            }
        };
        if leads {
            std::thread::sleep(GATHER);
            // Closed under the lock the others join it by: a task that comes
            // later starts a card of its own.
            let tasks = {
                let mut gathering = self
                    .gathering
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                *gathering = None;
                batch
                    .tasks
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clone()
            };
            batch.answer(ask(asker, &tasks));
        }
        let choice = match batch.wait(cancel) {
            Some(Choice::OneByOne) => ask(asker, &[task]),
            choice => choice.unwrap_or(Choice::Hidden),
        };
        match choice {
            Choice::Panel => SubagentPlace::Panel,
            Choice::Hidden | Choice::OneByOne => SubagentPlace::Hidden,
        }
    }
}

/// One task on the card: `& agent · its first line`.
fn task_line(agent: &str, prompt: &str) -> String {
    let first = prompt
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default()
        .trim();
    format!("& {agent} · {first}")
}

/// Put the card for `tasks` to the user. Dismissed, or answered in words of
/// their own, it is hidden: nothing runs where they did not say.
fn ask(asker: &UserAsker, tasks: &[String]) -> Choice {
    let t = termide_i18n::t();
    let hidden = t.settings_agent_subagent_run_hidden().to_string();
    let panel = t.settings_agent_subagent_run_panel().to_string();
    let one_by_one = t.agent_delegate_ask_one_by_one().to_string();
    let mut options = vec![
        QuestionOption {
            label: hidden,
            description: t.agent_delegate_ask_hidden_desc().to_string(),
        },
        QuestionOption {
            label: panel.clone(),
            description: t.agent_delegate_ask_panel_desc().to_string(),
        },
    ];
    if tasks.len() > 1 {
        options.push(QuestionOption {
            label: one_by_one.clone(),
            description: t.agent_delegate_ask_one_by_one_desc().to_string(),
        });
    }
    let question = Question {
        header: t.agent_delegate_ask_header().to_string(),
        question: t.agent_delegate_ask_question_fmt(&tasks.join("\n")),
        options,
        multi_select: false,
    };
    match asker.ask(vec![question]) {
        QuestionReply::Answered(answers) => {
            match answers.first().and_then(|answer| answer.chosen.first()) {
                Some(label) if *label == panel => Choice::Panel,
                Some(label) if *label == one_by_one => Choice::OneByOne,
                _ => Choice::Hidden,
            }
        }
        QuestionReply::Declined => Choice::Hidden,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use termide_agent_core::{question_channel, QuestionAnswer, QuestionEnvelope};

    fn pick(envelope: QuestionEnvelope, index: usize) -> Vec<String> {
        let question = &envelope.questions[0];
        let label = question.options[index].label.clone();
        let lines = question
            .question
            .lines()
            .skip(1)
            .map(str::to_string)
            .collect();
        envelope
            .reply
            .send(QuestionReply::Answered(vec![QuestionAnswer {
                chosen: vec![label],
                custom: None,
            }]))
            .unwrap();
        lines
    }

    /// Hand `prompts` over at once, each on a thread of its own.
    fn hand_over(
        asks: &Arc<PlaceAsks>,
        asker: &UserAsker,
        prompts: &[&str],
    ) -> Vec<std::thread::JoinHandle<SubagentPlace>> {
        prompts
            .iter()
            .map(|prompt| {
                let (asks, asker, prompt) = (Arc::clone(asks), asker.clone(), prompt.to_string());
                std::thread::spawn(move || {
                    asks.place("search", &prompt, Some(&asker), &CancelToken::new())
                })
            })
            .collect()
    }

    #[test]
    fn tasks_handed_over_together_get_one_card() {
        let asks = Arc::new(PlaceAsks::default());
        let (asker, cards) = question_channel(CancelToken::new());
        let calls = hand_over(&asks, &asker, &["find the loader\nin src", "read the docs"]);
        let card = cards.recv().unwrap();
        assert_eq!(card.questions[0].options.len(), 3);
        let mut tasks = pick(card, 1);
        tasks.sort();
        assert_eq!(
            tasks,
            ["& search · find the loader", "& search · read the docs"]
        );
        for call in calls {
            assert_eq!(call.join().unwrap(), SubagentPlace::Panel);
        }
        assert!(cards.try_recv().is_err(), "one card for both");
    }

    #[test]
    fn one_by_one_asks_for_each_task() {
        let asks = Arc::new(PlaceAsks::default());
        let (asker, cards) = question_channel(CancelToken::new());
        let calls = hand_over(&asks, &asker, &["one", "two"]);
        pick(cards.recv().unwrap(), 2);
        // Each task asks now, with no one-by-one of its own.
        let mut places = Vec::new();
        for answer in [0, 1] {
            let card = cards.recv().unwrap();
            assert_eq!(card.questions[0].options.len(), 2);
            pick(card, answer);
        }
        for call in calls {
            places.push(call.join().unwrap());
        }
        places.sort_by_key(|place| place.label());
        assert_eq!(places, [SubagentPlace::Hidden, SubagentPlace::Panel]);
    }

    #[test]
    fn a_dismissed_card_or_no_one_to_ask_runs_hidden() {
        let asks = PlaceAsks::default();
        assert_eq!(
            asks.place("search", "x", None, &CancelToken::new()),
            SubagentPlace::Hidden
        );
        let (asker, cards) = question_channel(CancelToken::new());
        let call = std::thread::spawn(move || {
            asks.place("search", "x", Some(&asker), &CancelToken::new())
        });
        let card = cards.recv().unwrap();
        assert_eq!(card.questions[0].options.len(), 2);
        card.reply.send(QuestionReply::Declined).unwrap();
        assert_eq!(call.join().unwrap(), SubagentPlace::Hidden);
    }
}

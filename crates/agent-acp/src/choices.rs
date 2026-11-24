//! The session's model and settings, changed one request at a time in the
//! order they were asked for. A restored session's choices and the user's
//! picks share one worker thread, so the agent ends on the latest; a pick
//! made while a restore waits supersedes the part of it not yet sent, and
//! a restore's reply that comes back after a pick of the same setting is
//! not taken as the setting in effect.

use std::collections::HashMap;
use std::sync::mpsc::{self, SendError, Sender};
use std::sync::{Arc, PoisonError};
use std::time::Duration;

use serde_json::json;
use termide_agent_core::{choice_wanted, ChoiceOutcome, MODEL_OPTION};

use crate::Shared;

/// The worker's queue and what telling a superseded restore needs.
#[derive(Default)]
pub(crate) struct Choices {
    /// The worker's queue, started on first use.
    jobs: Option<Sender<Job>>,
    /// The picks made so far.
    picks: u64,
    /// For each setting picked (the model under [`MODEL_OPTION`]), the
    /// count of picks at its latest one.
    picked: HashMap<String, u64>,
    /// The settled picks nobody waited on, for `choice_outcomes`.
    outcomes: Vec<ChoiceOutcome>,
}

enum Job {
    /// A restored session's choices, asked for once `after` picks were made:
    /// a setting picked later is left to that pick.
    Restore {
        choices: Vec<(String, String)>,
        after: u64,
    },
    /// The user's pick; its outcome goes to `reply` when the caller waits on
    /// it, else to [`Choices::outcomes`].
    Pick {
        id: String,
        value: String,
        reply: Option<Sender<Result<(), String>>>,
    },
}

impl Shared {
    fn choices(&self) -> std::sync::MutexGuard<'_, Choices> {
        self.choices.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Queue a restored session's choices behind whatever was asked before.
    pub(crate) fn restore_choices(self: &Arc<Self>, choices: Vec<(String, String)>) {
        let mut state = self.choices();
        let after = state.picks;
        self.send_job(&mut state, Job::Restore { choices, after });
    }

    /// Queue the user's pick of `value` for `id` behind whatever was asked
    /// before; a restore still waiting leaves `id` to it.
    pub(crate) fn pick(
        self: &Arc<Self>,
        id: String,
        value: String,
        reply: Option<Sender<Result<(), String>>>,
    ) {
        let mut state = self.choices();
        state.picks += 1;
        let count = state.picks;
        state.picked.insert(id.clone(), count);
        self.send_job(&mut state, Job::Pick { id, value, reply });
    }

    /// The user's pick, waited for here.
    pub(crate) fn pick_and_wait(self: &Arc<Self>, id: String, value: String) -> Result<(), String> {
        let (reply, outcome) = mpsc::channel();
        self.pick(id, value, Some(reply));
        outcome
            .recv()
            .unwrap_or_else(|_| Err("the agent's settings worker stopped".to_string()))
    }

    /// The picks settled since the last call, in the order they were made.
    pub(crate) fn take_choice_outcomes(&self) -> Vec<ChoiceOutcome> {
        std::mem::take(&mut self.choices().outcomes)
    }

    /// The worker holds the session weakly: it ends with the session, whose
    /// end drops the queue's sender.
    fn send_job(self: &Arc<Self>, state: &mut Choices, job: Job) {
        let job = match &state.jobs {
            Some(jobs) => match jobs.send(job) {
                Ok(()) => return,
                // The worker died (a panic): a new one takes over.
                Err(SendError(job)) => job,
            },
            None => job,
        };
        let (jobs, queue) = mpsc::channel::<Job>();
        let shared = Arc::downgrade(self);
        std::thread::spawn(move || {
            while let Ok(job) = queue.recv() {
                let Some(shared) = shared.upgrade() else {
                    return;
                };
                shared.run_job(job);
            }
        });
        // The receiver lives until the thread ends, which it cannot yet have.
        let _ = jobs.send(job);
        state.jobs = Some(jobs);
    }

    fn run_job(&self, job: Job) {
        match job {
            Job::Pick { id, value, reply } => {
                let result = self.switch(&id, &value, None);
                match reply {
                    Some(reply) => {
                        let _ = reply.send(result);
                    }
                    None => self
                        .choices()
                        .outcomes
                        .push(ChoiceOutcome { id, value, result }),
                }
            }
            Job::Restore { mut choices, after } => {
                // The model first: the settings offered can change with it,
                // and each is judged by what the agent states after it.
                choices.sort_by_key(|(id, _)| id != MODEL_OPTION);
                for (id, value) in choices {
                    if self.superseded(Some(after)).contains(&id) || !self.wanted(&id, &value) {
                        continue;
                    }
                    if let Err(error) = self.switch(&id, &value, Some(after)) {
                        log::warn!("cannot ask the agent for {id} = {value}: {error}");
                    }
                }
            }
        }
    }

    /// Whether a restored `id` = `value` is offered and not in effect now.
    fn wanted(&self, id: &str, value: &str) -> bool {
        let models = self
            .models
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let current = self
            .current_model
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let options = self
            .options
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        choice_wanted(id, value, &models, current.as_deref(), &options)
    }

    /// The settings picked after a restore asked at `restored`, the model's
    /// config option with the model; none for a pick (`None`), which only a
    /// later pick follows, sent after it.
    pub(crate) fn superseded(&self, restored: Option<u64>) -> Vec<String> {
        let Some(after) = restored else {
            return Vec::new();
        };
        let mut ids: Vec<String> = self
            .choices()
            .picked
            .iter()
            .filter(|(_, count)| **count > after)
            .map(|(id, _)| id.clone())
            .collect();
        if ids.iter().any(|id| id == MODEL_OPTION) {
            if let Some(option) = self
                .model_option
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
            {
                ids.push(option);
            }
        }
        ids
    }

    fn switch(&self, id: &str, value: &str, restored: Option<u64>) -> Result<(), String> {
        if id == MODEL_OPTION {
            self.select_model(value, restored)
        } else {
            self.set_config_option(id, value, restored)
        }
    }

    /// Switch the session's model: through its config option when the
    /// agent offers one, else through `session/set_model`. A restore's
    /// switch the user picked another model over meanwhile is not taken as
    /// the model in effect: the pick, sent next, is.
    fn select_model(&self, model_id: &str, restored: Option<u64>) -> Result<(), String> {
        let option = self
            .model_option
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        match option {
            Some(config_id) => self.set_config_option(&config_id, model_id, restored)?,
            None => {
                let session_id = self.session_id()?;
                self.request(
                    "session/set_model",
                    json!({ "sessionId": session_id, "modelId": model_id }),
                    Duration::from_secs(30),
                )?;
            }
        }
        if !self
            .superseded(restored)
            .iter()
            .any(|id| id == MODEL_OPTION)
        {
            *self
                .current_model
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(model_id.to_string());
        }
        Ok(())
    }

    /// Set config option `id` to `value` and take the options the agent
    /// replies with, which may change with it (the efforts a model offers);
    /// an agent that replies with none has the one value changed. For a
    /// restore, the settings picked meanwhile keep the values they have.
    pub(crate) fn set_config_option(
        &self,
        id: &str,
        value: &str,
        restored: Option<u64>,
    ) -> Result<(), String> {
        let session_id = self.session_id()?;
        let reply = self.request(
            "session/set_config_option",
            json!({ "sessionId": session_id, "configId": id, "value": value }),
            Duration::from_secs(30),
        )?;
        let keep = self.superseded(restored);
        match reply["configOptions"].as_array() {
            Some(options) => self.adopt_options_keeping(options, &keep),
            None if keep.iter().any(|kept| kept == id) => {}
            None => {
                if let Some(option) = self
                    .options
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .iter_mut()
                    .find(|option| option.id == id)
                {
                    option.current = value.to_string();
                }
            }
        }
        Ok(())
    }
}

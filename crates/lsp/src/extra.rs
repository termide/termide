//! Sources of completion, hover and code actions beside the language
//! servers — termide's plugins. [`LspManager`](crate::LspManager) asks them
//! along with the server for the file and hands the editor one answer: the
//! server's merged with theirs. Each source answers on a channel of its own,
//! and one that has not answered by [`EXTRA_WAIT`] is left out, so a slow
//! plugin never holds a completion back for long.

use std::path::Path;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use lsp_types::{
    CodeActionResponse, CompletionItem, CompletionList, CompletionResponse, Diagnostic,
    GotoDefinitionResponse, Hover, HoverContents, Location, MarkupContent, MarkupKind, Position,
    Range,
};

/// How long the extra sources have, counted from the request.
pub const EXTRA_WAIT: Duration = Duration::from_millis(1500);

/// What can answer beside the language servers.
pub trait ExtraProviders: Send + Sync {
    fn completion(
        &self,
        lang: &str,
        file_path: &Path,
        position: Position,
    ) -> Vec<Receiver<Option<CompletionResponse>>>;
    fn hover(
        &self,
        lang: &str,
        file_path: &Path,
        position: Position,
    ) -> Vec<Receiver<Option<Hover>>>;
    fn code_action(
        &self,
        lang: &str,
        file_path: &Path,
        range: Range,
        diagnostics: &[Diagnostic],
    ) -> Vec<Receiver<Option<CodeActionResponse>>>;
    fn definition(
        &self,
        _lang: &str,
        _file_path: &Path,
        _position: Position,
    ) -> Vec<Receiver<Option<GotoDefinitionResponse>>> {
        Vec::new()
    }
    fn references(
        &self,
        _lang: &str,
        _file_path: &Path,
        _position: Position,
    ) -> Vec<Receiver<Option<Vec<Location>>>> {
        Vec::new()
    }
}

static EXTRA: OnceLock<Arc<dyn ExtraProviders>> = OnceLock::new();

/// Set the extra sources, once for the process.
pub fn set_extra_providers(providers: Arc<dyn ExtraProviders>) {
    let _ = EXTRA.set(providers);
}

/// Whether extra sources are set: an editor keeps its language features on
/// without a server, as they may still answer.
#[must_use]
pub fn is_set() -> bool {
    EXTRA.get().is_some()
}

pub(crate) fn extra() -> Option<&'static Arc<dyn ExtraProviders>> {
    EXTRA.get()
}

/// One answer from `server` (if any) and `extras`: waits for each up to the
/// shared deadline on a thread, then merges what came with `merge`. With no
/// extra the server's channel is handed back as it is.
pub(crate) fn merged<T: Send + 'static>(
    server: Option<Receiver<Option<T>>>,
    extras: Vec<Receiver<Option<T>>>,
    merge: fn(Vec<T>) -> Option<T>,
) -> Option<Receiver<Option<T>>> {
    if extras.is_empty() {
        return server;
    }
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let deadline = Instant::now() + EXTRA_WAIT;
        let mut answers = Vec::new();
        // The server keeps its own pace, as it would without the extras.
        if let Some(server) = server {
            if let Ok(Some(answer)) = server.recv() {
                answers.push(answer);
            }
        }
        for extra in extras {
            let left = deadline.saturating_duration_since(Instant::now());
            if let Ok(Some(answer)) = extra.recv_timeout(left) {
                answers.push(answer);
            }
        }
        let _ = tx.send(merge(answers));
    });
    Some(rx)
}

pub(crate) fn merge_completions(answers: Vec<CompletionResponse>) -> Option<CompletionResponse> {
    if answers.is_empty() {
        return None;
    }
    let mut incomplete = false;
    let mut items: Vec<CompletionItem> = Vec::new();
    for answer in answers {
        match answer {
            CompletionResponse::Array(more) => items.extend(more),
            CompletionResponse::List(list) => {
                incomplete |= list.is_incomplete;
                items.extend(list.items);
            }
        }
    }
    Some(CompletionResponse::List(CompletionList {
        is_incomplete: incomplete,
        items,
    }))
}

pub(crate) fn merge_hovers(answers: Vec<Hover>) -> Option<Hover> {
    let texts: Vec<String> = answers
        .into_iter()
        .map(|hover| match hover.contents {
            HoverContents::Markup(markup) => markup.value,
            HoverContents::Scalar(marked) => marked_text(marked),
            HoverContents::Array(parts) => parts
                .into_iter()
                .map(marked_text)
                .collect::<Vec<_>>()
                .join("\n\n"),
        })
        .filter(|text| !text.trim().is_empty())
        .collect();
    (!texts.is_empty()).then(|| Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: texts.join("\n\n---\n\n"),
        }),
        range: None,
    })
}

fn marked_text(marked: lsp_types::MarkedString) -> String {
    match marked {
        lsp_types::MarkedString::String(text) => text,
        lsp_types::MarkedString::LanguageString(code) => {
            format!("```{}\n{}\n```", code.language, code.value)
        }
    }
}

pub(crate) fn merge_definitions(
    answers: Vec<GotoDefinitionResponse>,
) -> Option<GotoDefinitionResponse> {
    let mut locations = Vec::new();
    for answer in answers {
        match answer {
            GotoDefinitionResponse::Scalar(location) => locations.push(location),
            GotoDefinitionResponse::Array(more) => locations.extend(more),
            GotoDefinitionResponse::Link(links) => locations.extend(
                links
                    .into_iter()
                    .map(|link| Location::new(link.target_uri, link.target_selection_range)),
            ),
        }
    }
    (!locations.is_empty()).then_some(GotoDefinitionResponse::Array(locations))
}

pub(crate) fn merge_references(answers: Vec<Vec<Location>>) -> Option<Vec<Location>> {
    let locations: Vec<Location> = answers.into_iter().flatten().collect();
    (!locations.is_empty()).then_some(locations)
}

pub(crate) fn merge_actions(answers: Vec<CodeActionResponse>) -> Option<CodeActionResponse> {
    let actions: CodeActionResponse = answers.into_iter().flatten().collect();
    (!actions.is_empty()).then_some(actions)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(label: &str) -> CompletionItem {
        CompletionItem {
            label: label.into(),
            ..Default::default()
        }
    }

    #[test]
    fn answers_merge_into_one() {
        let merged = merge_completions(vec![
            CompletionResponse::Array(vec![item("a")]),
            CompletionResponse::List(CompletionList {
                is_incomplete: true,
                items: vec![item("b")],
            }),
        ]);
        let Some(CompletionResponse::List(list)) = merged else {
            panic!("a list");
        };
        assert!(list.is_incomplete);
        assert_eq!(list.items.len(), 2);
        let hover = merge_hovers(vec![
            Hover {
                contents: HoverContents::Scalar(lsp_types::MarkedString::String("server".into())),
                range: None,
            },
            Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: "plugin".into(),
                }),
                range: None,
            },
        ])
        .unwrap();
        let HoverContents::Markup(markup) = hover.contents else {
            panic!("markdown");
        };
        assert_eq!(markup.value, "server\n\n---\n\nplugin");
        assert!(merge_actions(Vec::new()).is_none());
    }

    #[test]
    fn a_slow_extra_is_left_out() {
        let (server_tx, server_rx) = mpsc::channel();
        let (_slow_tx, slow_rx) = mpsc::channel::<Option<Vec<i32>>>();
        server_tx.send(Some(vec![1])).unwrap();
        let rx = merged(Some(server_rx), vec![slow_rx], |answers| {
            Some(answers.into_iter().flatten().collect())
        })
        .unwrap();
        assert_eq!(rx.recv().unwrap(), Some(vec![1]));
    }
}

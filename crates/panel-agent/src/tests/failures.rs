//! Failed requests: the failure card, waits and retries.

use super::*;

#[test]
fn a_failed_run_offers_what_fits_and_tries_again_on_the_same_request() {
    let mut panel = panel(vec![limited(), reply("done")]);
    panel.send("do it".to_string());
    settle(&mut panel);

    let rows = failure_rows(&panel);
    assert!(rows[0].starts_with("Wait until"), "{rows:?}");
    assert!(rows.iter().any(|row| row == "Try again now"), "{rows:?}");
    assert!(
        !rows.iter().any(|row| row.contains("another agent")),
        "the card switches nothing: the status strip does: {rows:?}"
    );
    let now = rows.iter().position(|row| row == "Try again now").unwrap();
    panel.apply_form_action(ChoiceAction::Chosen(now));
    settle(&mut panel);

    let items = panel.transcript().items();
    let users = items
        .iter()
        .filter(|item| matches!(item, Item::User { .. }))
        .count();
    assert_eq!(
        users, 1,
        "the request is not sent a second time as a message"
    );
    assert!(items
        .iter()
        .any(|item| matches!(item, Item::Assistant { text, error: None, .. } if text == "done")));
    assert!(panel.pending.is_none());
}

#[test]
fn waiting_policy_tries_again_unasked_once_the_wait_is_over() {
    let mut panel = panel(vec![limited(), reply("done")]);
    panel.on_limit = LimitPolicy::Wait;
    panel.send("do it".to_string());
    settle(&mut panel);

    assert!(panel.pending.is_none(), "nothing is asked");
    let wait = panel.retry_wait.expect("the reset is waited for");
    assert!(wait.wall > crate::failure::unix_now() + 3_000);
    assert!(panel
        .retry_wait_text()
        .is_some_and(|text| text.contains("Usage limit reached")));
    // The reset comes.
    panel.retry_wait.as_mut().unwrap().at = Instant::now();
    panel.tick();
    settle(&mut panel);

    assert!(panel.retry_wait.is_none());
    assert!(panel
        .transcript()
        .items()
        .iter()
        .any(|item| matches!(item, Item::Assistant { text, error: None, .. } if text == "done")));
}

#[test]
fn a_new_request_takes_over_from_a_wait() {
    let mut panel = panel(vec![limited(), reply("other")]);
    panel.on_limit = LimitPolicy::Wait;
    panel.send("do it".to_string());
    settle(&mut panel);
    assert!(panel.retry_wait.is_some());

    panel.send("something else".to_string());
    assert!(panel.retry_wait.is_none());
    settle(&mut panel);
    assert!(panel.transcript().items().iter().any(
        |item| matches!(item, Item::Notice { text, .. } if text.contains("no longer waiting"))
    ));
}

#[test]
fn continue_tries_a_failed_request_again() {
    let mut panel = panel(vec![limited(), reply("done")]);
    panel.on_limit = LimitPolicy::Stop;
    panel.send("do it".to_string());
    settle(&mut panel);
    assert!(panel.pending.is_none(), "the stop policy asks nothing");

    type_text(&mut panel, "/continue");
    panel.submit();
    settle(&mut panel);
    assert!(panel
        .transcript()
        .items()
        .iter()
        .any(|item| matches!(item, Item::Assistant { text, error: None, .. } if text == "done")));
}

#[test]
fn dismissing_the_failure_card_ends_the_goal() {
    let mut panel = panel(vec![limited()]);
    type_text(&mut panel, "/goal get it done");
    panel.submit();
    settle(&mut panel);
    assert!(panel.goal_task.is_some(), "the goal waits on the card");
    assert!(matches!(panel.pending, Some(Pending::Failure { .. })));

    panel.apply_form_action(ChoiceAction::Cancelled);
    assert!(panel.goal_task.is_none());
    assert!(panel.pending.is_none());
}

/// A card left open keeps up with the clock: its countdown moves, and once
/// the reset has gone by it offers no wait for a moment already behind — the
/// request is simply tried again.
#[test]
fn an_open_failure_card_follows_the_clock_and_drops_a_reset_past() {
    let mut panel = panel(vec![limited(), reply("done")]);
    panel.send("do it".to_string());
    settle(&mut panel);

    let (rows, detail) = failure_card(&panel);
    assert!(rows[0].starts_with("Wait until"), "{rows:?}");
    assert!(detail.contains("Resets at"), "{detail}");
    let first_wait = rows[0].clone();

    // The reset moves further off: the row and the detail say so.
    set_reset(&mut panel, crate::failure::unix_now() + 7_200);
    panel.failure_card_shown = None;
    assert!(panel.refresh_failure_card(), "the stated time moved");
    let (rows, detail) = failure_card(&panel);
    assert_ne!(rows[0], first_wait, "{rows:?}");
    assert!(detail.contains("Resets at"), "{detail}");

    // The reset comes and goes while the card stands open.
    set_reset(&mut panel, crate::failure::unix_now() - 60);
    panel.failure_card_shown = None;
    assert!(panel.refresh_failure_card());
    let (rows, detail) = failure_card(&panel);
    assert!(
        !rows.iter().any(|row| row.starts_with("Wait until")),
        "a reset already past is not waited for: {rows:?}"
    );
    assert!(rows.iter().any(|row| row == "Try again now"), "{rows:?}");
    assert!(detail.contains("has passed"), "{detail}");

    // The row picked is the one that does it: the request goes again.
    let now = rows.iter().position(|row| row == "Try again now").unwrap();
    panel.apply_form_action(ChoiceAction::Chosen(now));
    settle(&mut panel);
    assert!(panel.pending.is_none());
    assert!(panel
        .transcript()
        .items()
        .iter()
        .any(|item| matches!(item, Item::Assistant { text, error: None, .. } if text == "done")));
}

/// The card's text moves once a second, not on every tick of the loop.
#[test]
fn the_failure_card_counts_down_once_a_second() {
    let mut panel = panel(vec![limited()]);
    panel.send("do it".to_string());
    settle(&mut panel);
    let (rows, _) = failure_card(&panel);

    // The same second: nothing is rebuilt, however the moment is moved.
    set_reset(&mut panel, crate::failure::unix_now() + 7_200);
    panel.failure_card_shown = Some(crate::failure::unix_now());
    assert!(
        !panel.refresh_failure_card(),
        "a second already rebuilt is not rebuilt again"
    );
    assert_eq!(failure_card(&panel).0, rows, "the text stands still");

    // A new second takes the change up.
    panel.failure_card_shown = None;
    assert!(panel.refresh_failure_card(), "a new second rebuilds it");
    assert_ne!(failure_card(&panel).0, rows, "the stated time moved");
}

/// A card asked about a limit whose reset was already in the past when the
/// run failed never offers to wait for it.
#[test]
fn a_reset_already_past_offers_no_wait() {
    let mut failed = limited();
    failed.failure.as_mut().unwrap().resets_at = Some(crate::failure::unix_now() - 120);
    let mut panel = panel(vec![failed]);
    panel.send("do it".to_string());
    settle(&mut panel);

    let (rows, detail) = failure_card(&panel);
    assert!(
        !rows.iter().any(|row| row.starts_with("Wait until")),
        "{rows:?}"
    );
    assert!(detail.contains("has passed"), "{detail}");
}

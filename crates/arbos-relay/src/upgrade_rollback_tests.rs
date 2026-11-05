//! Failed-upgrade rollback (Oct 19–25 orphans). Every journalled orphan came from an upgrade that hit
//! a Daytona toolbox HTTP 502 and logged "upgrade rolled back". In five, the row ended on the
//! half-filled new sandbox and the full source ran on with no row (a user's whole project was left
//! only on the unreferenced source); in two, the row stayed on the source but the new sandbox was
//! never deleted. At 21:35:17 on Oct 25 a wake bounced the box in the same second as the rollback.
//!
//! The upgrade runs here for real against a stand-in Daytona with a 502 at one step. After every
//! case: the row is on the source with its own token, the source runs, and every sandbox the stand-in
//! still has is either the row's or recorded in `box_history`. Test data only.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use arbos_core::ids::now_ms;

use crate::boxes;
use crate::upgrade::{self, Source, Staged};
use crate::upgrade_box_fake::{
    assert_rolled_back_to_source, new_box, row, setup, upgrade_until_done, SRC, USER, WEEK_MS,
};

#[tokio::test]
async fn a_502_during_pack_keeps_the_source_and_makes_no_new_box() {
    let (state, fake) = setup(&[(SRC, "tar --null")]).await;
    upgrade_until_done(&state).await;
    assert_rolled_back_to_source(&state, &fake);
    assert_eq!(fake.created.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_502_during_unpack_before_any_data_deletes_the_empty_new_box() {
    let (state, fake) = setup(&[("new", "df -Pm")]).await;
    upgrade_until_done(&state).await;
    assert_rolled_back_to_source(&state, &fake);
    let new = new_box(&fake);
    assert_eq!(*fake.deleted.lock().unwrap(), vec![new.clone()]);
    assert!(state.db.box_history_entry(&new).unwrap().is_none());
}

/// Two of the orphans: the new box got part of `/work`, then a 502. It is not deleted (it holds
/// user data), not left running, and recorded so the sweeper deletes it after the window.
#[tokio::test]
async fn a_502_during_unpack_after_data_keeps_the_new_box_stopped_and_recorded() {
    let (state, fake) = setup(&[("new", "tar -xzf")]).await;
    upgrade_until_done(&state).await;
    assert_rolled_back_to_source(&state, &fake);
    let new = new_box(&fake);
    assert!(fake.deleted.lock().unwrap().is_empty());
    assert_eq!(fake.state(&new), Some("stopped"));
    let (delete_after, partial) = state.db.box_history_entry(&new).unwrap().expect("recorded");
    assert!(partial);
    assert!(delete_after > now_ms() + WEEK_MS - 60_000);
    assert!(
        state.db.latest_box_history(USER).unwrap().is_none(),
        "a half-carried box is never offered for repair"
    );
}

/// The delete of an empty new box answers 502 too: it is recorded, due now, for the sweeper.
#[tokio::test]
async fn a_502_on_the_cleanup_delete_never_orphans_anything() {
    let (state, fake) = setup(&[("new", "df -Pm"), ("new", "DELETE ")]).await;
    upgrade_until_done(&state).await;
    assert_rolled_back_to_source(&state, &fake);
    let new = new_box(&fake);
    let (delete_after, _) = state.db.box_history_entry(&new).unwrap().expect("recorded");
    assert!(delete_after <= now_ms());
    assert!(state
        .db
        .box_history_due()
        .unwrap()
        .contains(&(USER.to_owned(), new)));
}

/// The five source orphans' shape: a 502 once the row names the new box (its start). The row goes back to the source; the new box, which holds the carried
/// `/work`, is kept stopped and recorded.
#[tokio::test]
async fn a_502_during_the_swap_puts_the_row_back_on_the_source() {
    let (state, fake) = setup(&[("new", "/start")]).await;
    upgrade_until_done(&state).await;
    assert_rolled_back_to_source(&state, &fake);
    let new = new_box(&fake);
    assert!(fake.deleted.lock().unwrap().is_empty());
    assert_eq!(fake.state(&new), Some("stopped"));
    assert!(
        state
            .db
            .box_history_entry(&new)
            .unwrap()
            .expect("recorded")
            .1
    );
}

/// Oct 25 21:35:17: a wake read the row while it named the new box, bounced that sandbox ("sandbox
/// is started but the box is silent; restarting it") and finished after the rollback had put the
/// source back. Its write-back put the half-filled box into the row; the full source ran on with
/// no row. The wake must not write a row that moved on, and the new box must not keep running.
#[tokio::test]
async fn a_wake_racing_the_rollback_does_not_write_the_new_box_back() {
    let (state, fake) = setup(&[]).await;
    let source = row(SRC, "hash-src");
    let new = row("new-race", "hash-new");
    fake.set(SRC, "stopped");
    fake.set("new-race", "started");
    state
        .db
        .insert_box_history(&source, now_ms() + WEEK_MS)
        .unwrap();
    state.db.upsert_box(&new).unwrap();
    let release = Arc::new(tokio::sync::Notify::new());
    *fake.start_gate.lock().unwrap() = Some(("new-race".into(), release.clone()));

    let st = state.clone();
    let wake = tokio::spawn(async move { boxes::start_box(&st, USER).await });
    tokio::time::timeout(Duration::from_secs(10), fake.gate_reached.notified())
        .await
        .expect("the wake reached the new box's start");

    let dt = state.daytona.as_ref().unwrap();
    let staged = Staged {
        row: &new,
        keep: true,
        swapped: true,
    };
    upgrade::rollback(
        &state,
        dt,
        USER,
        &Source::CurrentBox,
        Some(&source),
        Some(staged),
    )
    .await;
    release.notify_one();
    let woke = tokio::time::timeout(Duration::from_secs(10), wake)
        .await
        .expect("wake finished")
        .unwrap();

    assert_rolled_back_to_source(&state, &fake);
    assert!(woke.is_err(), "the wake must notice the row moved on");
    assert_eq!(fake.state("new-race"), Some("stopped"));
    assert!(
        state
            .db
            .box_history_entry("new-race")
            .unwrap()
            .expect("recorded")
            .1
    );
}

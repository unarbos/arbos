//! Disk room around the `/work` carry (gh_268993031, 2026-11-05). That upgrade packed `/work` onto
//! the source box's own disk, which had 365 MiB free; part 80 was a lone 356.9 MiB file, its tarball
//! filled the disk and `head` cutting the first piece failed with ENOSPC. Three things were wrong:
//! the source's free space was never checked, the failed pack left 389 MiB of its own staging on the
//! box (which then made agentd release its ballast), and the destination's room was only checked
//! after the pack — so a snapshot too small for the carry is refused after ~22 minutes of packing,
//! a stop of the old sandbox and a create of the new one.
//!
//! The upgrade runs here for real against the stand-in in [`crate::upgrade_box_fake`]. Test data only.

use crate::state::AppState;
use crate::upgrade_box_fake::{refusal, setup, upgrade_until_done, Disk, Fake, SRC, USER};

/// Nothing happened to the user's box: the row still names the source, the source still runs and
/// no new sandbox was made.
///
/// The two refusal cases also arm a 502 on the start of any new box. Nothing reaches it while the
/// measure step refuses; it is there so that a carry which is *not* refused rolls back in a moment
/// and the case fails on the assertion below, instead of waiting out the new box's hello.
fn assert_box_untouched(state: &AppState, fake: &Fake) {
    let r = state.db.user_box(USER).unwrap().expect("row");
    assert_eq!(
        r.sandbox_id.as_deref(),
        Some(SRC),
        "the row left the source"
    );
    assert_eq!(r.snapshot.as_deref(), Some("arbos-agent-test-old"));
    assert_eq!(
        fake.state(SRC),
        Some("started"),
        "the source is not running"
    );
    assert_eq!(
        fake.created.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "a new box was made for a carry that cannot work"
    );
    assert!(
        !fake.quiesced(),
        "the box's agentd was stopped for a carry that cannot work"
    );
    assert!(!fake.ran("tar --null"), "the pack started");
}

/// The source stages every part on its own disk, so a box without room for its biggest part is
/// refused while it is still serving. 600 MiB biggest file against 365 MiB free.
#[tokio::test]
async fn a_source_without_room_for_its_biggest_part_is_refused_before_agentd_is_stopped() {
    let (state, fake) = setup(&[("new", "/start")]).await;
    fake.set_disk(Disk {
        work_mib: 700,
        source_free_mib: 365,
        max_file_bytes: 600 * 1024 * 1024,
        ..Disk::default()
    });
    upgrade_until_done(&state).await;
    let reason = refusal(&state).await.expect("refused with a reason");
    assert!(
        reason.contains("the source box has 365 MiB free"),
        "{reason}"
    );
    assert!(
        reason.contains("600"),
        "the biggest part is named: {reason}"
    );
    assert!(reason.contains("too large to carry"), "{reason}");
    assert_box_untouched(&state, &fake);
}

/// The relay knows the target snapshot's disk size before it packs anything, so a snapshot that
/// cannot hold the carry costs nothing: 700 MiB of `/work` with a 600 MiB part needs more than a
/// 1 GiB box can give. Before this, the same arithmetic ran only on the new box, after the pack.
#[tokio::test]
async fn a_snapshot_too_small_for_the_carry_is_refused_before_the_pack() {
    let (state, fake) = setup(&[("new", "/start")]).await;
    fake.set_disk(Disk {
        work_mib: 700,
        max_file_bytes: 600 * 1024 * 1024,
        snapshot_disk_gib: Some(1),
        ..Disk::default()
    });
    upgrade_until_done(&state).await;
    let reason = refusal(&state).await.expect("refused with a reason");
    assert!(reason.contains("/work is 700 MiB"), "{reason}");
    assert!(
        reason.contains("a fresh box from the target snapshot has about 768 MiB free"),
        "{reason}"
    );
    assert!(reason.contains("too large to carry"), "{reason}");
    assert_box_untouched(&state, &fake);
}

/// The new checks must not refuse a carry that fits: with a 40 GiB snapshot the same box packs.
/// The pack is cut short by a 502 at the first tar so the case stays a unit of work.
#[tokio::test]
async fn a_carry_that_fits_is_packed_and_not_refused() {
    let (state, fake) = setup(&[(SRC, "tar --null")]).await;
    fake.set_disk(Disk {
        work_mib: 700,
        max_file_bytes: 600 * 1024 * 1024,
        snapshot_disk_gib: Some(40),
        ..Disk::default()
    });
    upgrade_until_done(&state).await;
    assert_eq!(refusal(&state).await, None, "a carry that fits was refused");
    assert!(
        fake.quiesced() && fake.ran("tar --null"),
        "the pack never ran"
    );
}

/// A pack that fails takes its own staging with it. The ENOSPC case left `part-80.tgz` (372 MB),
/// `work.list` (26 MB) and a piece behind, because the early return skipped the cleanup.
#[tokio::test]
async fn a_failed_pack_leaves_no_staging_on_the_box() {
    let (state, fake) = setup(&[(SRC, "tar --null")]).await;
    upgrade_until_done(&state).await;
    assert!(fake.ran("tar --null"), "the pack never ran");
    assert_eq!(
        fake.staging_clears(),
        1,
        "the failed pack left its staging on the box"
    );
}

/// The listing step writes `work.list` into the same directory, so its own failure clears it too.
#[tokio::test]
async fn a_failed_listing_leaves_no_staging_on_the_box() {
    let (state, fake) = setup(&[(SRC, "-print0 >")]).await;
    upgrade_until_done(&state).await;
    assert!(
        !fake.ran("tar --null"),
        "the pack ran after a failed listing"
    );
    assert_eq!(
        fake.staging_clears(),
        1,
        "the failed listing left its staging on the box"
    );
}

/// A carry that packs cleanly still clears the staging, exactly once.
#[tokio::test]
async fn a_pack_that_succeeds_clears_the_staging_once() {
    let (state, fake) = setup(&[("new", "df -Pm")]).await;
    upgrade_until_done(&state).await;
    assert!(fake.ran("tar --null"), "the pack never ran");
    assert_eq!(fake.staging_clears(), 1);
}

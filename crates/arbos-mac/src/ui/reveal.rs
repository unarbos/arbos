//! Where a jump brings a message to rest, and the one mechanism that puts it
//! there. Shared by every list a reveal can land in — the chat transcript under
//! its floating header, a search results list under a bar — so the two never
//! grow two conventions for the same behaviour.
//!
//! **The rule.** A revealed message rests with the top edge of its row one inset
//! below the top edge of the scrolling viewport, where
//!
//! ```text
//! inset = how far the overlay's dimming reaches into the list
//!       + one unit of the list's own vertical rhythm
//! ```
//!
//! Measured from the **viewport's** top edge — the list's content box — not from
//! the window edge and not from the bottom of the overlay. A transcript that runs
//! under a floating header has the two in the same place; a list under a bar that
//! sits in flow above it starts below the bar, and the inset is measured from
//! there.
//!
//! The two terms do two different jobs. **The dimming is the floor**: a row inside
//! the overlay's influence is the faintest thing on screen, which is the one place
//! the message a search just found must not land. **The rhythm is the judgement**:
//! a row whose first line begins exactly where the dimming stops still reads as
//! jammed under the chrome, and one gap of the list's own rhythm lets the message
//! above show its last line *through* the dimming — so the hit reads as part of a
//! conversation and is the first fully bright row, which is where the eye goes.
//!
//! Neither term is a number anyone writes down: both are read from where they
//! already live. A literal that has to be kept in step with a header's size by
//! hand is how this regresses.
//!
//! A share of the viewport was the other candidate and is a rule that is right by
//! luck: centring clears the overlay at some viewport heights and not others. The
//! iPhone does centre (`ChatView.revealQuoted`, `anchor: .center`), which is the
//! right phone answer because its viewport makes centre and this inset nearly the
//! same displacement; on a desktop pane centre throws away half the screen of
//! forward reading, and a hit is read downward from the hit.
//!
//! **Where the inset cannot apply.** It is a request, not a requirement, and both
//! ends of a conversation clamp it rather than fake it.
//!
//! - The first message in a list has nothing above it to put in the band, so it
//!   cannot be lifted clear: the walk runs out of rows and lands exactly where a
//!   reveal landed before any of this. No better, no worse.
//! - A hit near the end rests *lower* than the inset, because keeping the last
//!   page on screen wins over scrolling into empty space past it. That is already
//!   clear of the overlay, so there is nothing to correct.
//! - A message taller than the viewport anchors its top, with the same inset:
//!   reading starts at its first line. This is the case a centred reveal gets
//!   actively wrong, dropping the reader into the middle of the message.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{canvas, prelude::*, px, Bounds, IntoElement};

/// The resting inset, from its two terms. Spelled as a function so both of them
/// have to be named at the call site, and so there is one place the rule lives.
///
/// `dimming` is how far the overlay's dimming reaches into the list — the
/// header's fade band, a bar's shadow — read from the overlay, never written
/// down. `rhythm` is one unit of the list's own vertical rhythm: the gap it
/// already opens between two speakers, or between two rows.
pub const fn top_inset(dimming: f32, rhythm: f32) -> f32 {
    dimming + rhythm
}

/// The inset a reveal still owes the row it landed on.
///
/// Shared between the view and that row's element, because the request has to be
/// made from inside the list's prepaint, and cleared there so one reveal corrects
/// once and the reader can scroll away afterwards.
#[derive(Clone, Default)]
pub struct RevealInset(Rc<Cell<Option<usize>>>);

impl RevealInset {
    /// A reveal landed on row `ix` and still owes it the inset. Call this beside
    /// the scroll that put the row at the viewport's top edge.
    pub fn owed(&self, ix: usize) {
        self.0.set(Some(ix));
    }

    /// Nothing is being revealed any more.
    pub fn clear(&self) {
        self.0.set(None);
    }

    /// The child row `ix` carries while it is the row owed `inset`, and nothing
    /// otherwise. Add it to the row's own frame — the element the list lays out
    /// as that item — so the bounds it reads are the item's.
    ///
    /// It asks for the inset the way GPUI's `List` takes it: an autoscroll
    /// request whose top edge sits `inset` above the row's own. `List`'s prepaint
    /// answers by walking into the rows above until the offset it lands on is
    /// non-negative, which both fills the inset with the messages above the hit
    /// and clamps at the first row when there are none. It corrects inside the
    /// same frame, so there is no second step to see.
    pub fn row_request(&self, ix: usize, inset: f32) -> Option<impl IntoElement> {
        if self.0.get() != Some(ix) {
            return None;
        }
        let pending = self.0.clone();
        Some(
            canvas(
                move |bounds, window, _| {
                    if pending.get() != Some(ix) {
                        return;
                    }
                    pending.set(None);
                    window.request_autoscroll(Bounds::from_corners(
                        gpui::point(bounds.left(), bounds.top() - px(inset)),
                        gpui::point(bounds.right(), bounds.top()),
                    ));
                },
                |_, _, _, _| {},
            )
            .absolute()
            .size_full(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both terms, named, added — and the shape of the rule rather than a number
    /// of its own, so a header or a bar that changes size carries the inset with
    /// it.
    #[test]
    fn the_inset_is_the_dimming_plus_one_unit_of_rhythm() {
        assert_eq!(top_inset(66.0, 20.0), 86.0);
        assert!(
            top_inset(66.0, 20.0) > 66.0,
            "the dimming is a floor the inset clears, not a target it meets"
        );
    }

    /// One reveal corrects once: the row it landed on takes the request, every
    /// other row takes nothing, and the debt is paid when the element is built.
    #[test]
    fn only_the_revealed_row_is_owed_the_inset() {
        let inset = RevealInset::default();
        assert!(inset.row_request(3, 86.0).is_none(), "nothing revealed");
        inset.owed(3);
        assert!(inset.row_request(2, 86.0).is_none(), "another row");
        assert!(inset.row_request(3, 86.0).is_some(), "the revealed row");
        inset.clear();
        assert!(inset.row_request(3, 86.0).is_none());
    }
}

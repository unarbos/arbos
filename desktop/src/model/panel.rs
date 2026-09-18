//! The side panel's own tabs: what is open in the drawer on the right, which
//! of them is in front, and whether the drawer is open at all.
//!
//! One drawer per project, so switching tabs at the top of the window switches
//! this with it. Closed unless the person opened it: a terminal or a job the
//! agent starts becomes a tab here and is *listed*, but it never opens the
//! drawer and never takes the tab in front. Only the person does that.
//!
//! A tab holds an address, never content — [`PanelTab::Surface`] names a
//! [`SurfaceId`], and the surface it names is the one thing that knows what is
//! behind it. A tab whose surface has gone is dropped by
//! [`Panel::retain_surfaces`] rather than kept as a row the kernel cannot
//! account for (F-137).

use crate::model::surface::SurfaceId;

/// The drawer's width when the person has not set one, and the range a drag
/// may take it to. Measured off Cursor at a 1440-wide window
/// (`internal/cursor-side-panel-measured.md`): its side panel opens at half
/// the space beside the sidebar — 592 — and clamps between 377 and 766, with
/// the chat never squeezed under 418.
///
/// Project and every other tab share that wide measure. A 280-pt Project
/// tab next to a 592-pt Terminal read as two different drawers; they are
/// one drawer, so they open at the same width. A drag still remembers its
/// own number and wins over this default.
pub const PAGE_WIDTH: f32 = 592.;
pub const SURFACE_WIDTH: f32 = 592.;
pub const MIN_WIDTH: f32 = 377.;
pub const MAX_WIDTH: f32 = 766.;

/// The chat's floor. Cursor's divider clamps at 418 and never squeezes the
/// chat under it; this app has no left sidebar, so the same floor leaves more
/// room, not less.
pub const CHAT_MIN_WIDTH: f32 = 418.;

/// Whether a document tab lets him type into the file.
///
/// **Jacob's ruling, 2026-09-17: his version wins and the agent is refused.**
/// The window owns three of the four rules — the file is always his to type
/// in, nothing is reloaded under him, and his save is a compare-and-swap.
/// The fourth is the kernel's `claim` frame: while the editor holds unsaved
/// edits, the agent's write to that path is refused. That frame exists, so
/// a file tab is an editor.
pub const DOCUMENTS_EDITABLE: bool = true;

/// Which side opened a surface. The window knows its own clicks, and since
/// [#461](https://github.com/unarbos/arbos/pull/461) a `board` frame says as
/// well: `by: user` for a shell a client asked for or a `terminal` call the
/// agent marked as the person's request, `by: agent` for the agent's own work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenedBy {
    /// A click in the window, `⌘T` and a card, or a frame that says `user`:
    /// the drawer opens and the tab comes to the front.
    User,
    /// The agent's own work: listed, and nothing moves.
    Agent,
}

impl OpenedBy {
    /// What a `board` frame's `by` means here.
    ///
    /// Anything that is not plainly `user` is treated as the agent's. A kernel
    /// from before that field sends nothing at all, and reading silence as
    /// "the person asked for this" would let an old kernel throw the drawer
    /// open over what he is typing — the guess this whole rule exists to
    /// avoid. Unknown is not a third behaviour; it is the quiet one.
    pub fn from_frame(by: &str) -> Self {
        match by {
            "user" => Self::User,
            _ => Self::Agent,
        }
    }
}

/// What one tab holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelTab {
    /// The project's `.arbos/`: its agents, their processes, the resources
    /// they hold and the project page. Always the first tab, and never
    /// closed — it is what "open the project" opens, and the one place the
    /// panel can always fall back to.
    Project,
    /// A terminal, a job's output, a browser page, a document.
    Surface(SurfaceId),
    /// A tab with nothing in it yet: the four cards. The number keeps two of
    /// them apart.
    New(u64),
}

/// One project's drawer.
#[derive(Debug, Clone)]
pub struct Panel {
    /// Whether it is open. Default closed, and nothing but the person opens
    /// it.
    pub open: bool,
    /// The width the person dragged it to, if they ever did. `None` takes
    /// [`SURFACE_WIDTH`] for every tab, Project included.
    pub width: Option<f32>,
    tabs: Vec<PanelTab>,
    active: usize,
    next_new: u64,
}

impl Default for Panel {
    fn default() -> Self {
        Self {
            open: false,
            width: None,
            tabs: vec![PanelTab::Project],
            active: 0,
            next_new: 1,
        }
    }
}

impl Panel {
    pub fn tabs(&self) -> &[PanelTab] {
        &self.tabs
    }

    /// Which tab is in front. Clamped on the way out: a tab dropped from under
    /// the index must not leave a row lit that nobody can see.
    pub fn active(&self) -> usize {
        self.active.min(self.tabs.len().saturating_sub(1))
    }

    pub fn active_tab(&self) -> PanelTab {
        self.tabs
            .get(self.active())
            .copied()
            .unwrap_or(PanelTab::Project)
    }

    /// How wide to draw: what the person dragged it to, held inside the range
    /// a drag may take, or the shared default every tab opens at.
    pub fn width(&self) -> f32 {
        self.width
            .unwrap_or(SURFACE_WIDTH)
            .clamp(MIN_WIDTH, MAX_WIDTH)
    }

    /// Remember a dragged width. Held inside the range a drag may take, so a
    /// pointer past either end cannot leave a drawer the chat cannot live
    /// beside.
    pub fn set_width(&mut self, width: f32) {
        self.width = Some(width.clamp(MIN_WIDTH, MAX_WIDTH));
    }

    /// ⌘\\ and the four-box: grow to the space the window can spare, or
    /// return to the default if it is already there. Never moves the tab
    /// into the chat column — that cloned a Terminal over the conversation.
    pub fn toggle_expanded_width(&mut self, available: f32) {
        let max = available.clamp(MIN_WIDTH, MAX_WIDTH);
        let current = self.width();
        self.width = Some(if current >= max - 8.0 {
            SURFACE_WIDTH
        } else {
            max
        });
    }

    pub fn select(&mut self, ix: usize) {
        if ix < self.tabs.len() {
            self.active = ix;
        }
    }

    /// The tab holding `id`, if one does.
    pub fn position_of(&self, id: SurfaceId) -> Option<usize> {
        self.tabs
            .iter()
            .position(|tab| *tab == PanelTab::Surface(id))
    }

    /// A surface the agent opened, or the person did. It becomes a tab if it
    /// is not one already, and the two flags are what tell the routes apart:
    ///
    /// - the agent opens something: neither — it is listed and nothing moves;
    /// - the agent's `focus` says look at this: `select`, so it is what the
    ///   drawer shows when the person opens it, but it does not open it;
    /// - the person clicks a row or a card: both.
    ///
    /// Only the person's route sets `open`. The window cannot tell a terminal
    /// the agent started for itself from one he asked for in prose, so it
    /// never guesses (`docs/side-panels-design.md`, "The panel never opens
    /// itself").
    ///
    /// The person's open fills an empty tab in front rather than making a
    /// second one, which is what `⌘T` and then a card on it reads as.
    pub fn add_surface(&mut self, id: SurfaceId, select: bool, open: bool) -> usize {
        let at = match self.position_of(id) {
            Some(at) => at,
            None => {
                let empty_in_front = select
                    && matches!(self.active_tab(), PanelTab::New(_))
                    && self.active() < self.tabs.len();
                if empty_in_front {
                    let at = self.active();
                    self.tabs[at] = PanelTab::Surface(id);
                    at
                } else {
                    // Beside the tab in front rather than at the end, which
                    // is where Cursor puts a new one.
                    let at = (self.active() + 1).min(self.tabs.len());
                    self.tabs.insert(at, PanelTab::Surface(id));
                    at
                }
            }
        };
        if select {
            self.active = at;
        }
        self.open |= open;
        at
    }

    /// A new empty tab, beside the one in front — where Cursor opens one.
    /// What `⌘T` does with the drawer focused.
    pub fn new_tab(&mut self) -> usize {
        let id = self.next_new;
        self.next_new += 1;
        let at = (self.active() + 1).min(self.tabs.len());
        self.tabs.insert(at, PanelTab::New(id));
        self.active = at;
        self.open = true;
        self.active
    }

    /// Close the tab at `ix`. The project tab does not close: it is the
    /// drawer's floor. Closing what was in front lands on the tab to its
    /// right, or the one to its left when it was last.
    pub fn close(&mut self, ix: usize) {
        if ix == 0 || ix >= self.tabs.len() {
            return;
        }
        let was = self.active();
        self.tabs.remove(ix);
        // The project tab is never removed, so there is always one left.
        let last = self.tabs.len() - 1;
        self.active = match was.cmp(&ix) {
            std::cmp::Ordering::Less => was,
            // The tab that slid into its place, or its left neighbour when
            // the one closed was the last.
            std::cmp::Ordering::Equal => ix.min(last),
            std::cmp::Ordering::Greater => was - 1,
        };
    }

    /// Close every tab holding a surface. What "close the panel's tabs"
    /// means when a project is being put away.
    pub fn close_surfaces(&mut self) {
        self.tabs.retain(|tab| !matches!(tab, PanelTab::Surface(_)));
        self.active = self.active.min(self.tabs.len().saturating_sub(1));
    }

    /// Drop tabs whose surface is no longer there. Called after any change to
    /// the project's surfaces, so a row can never outlive what it points at.
    pub fn retain_surfaces(&mut self, alive: impl Fn(SurfaceId) -> bool) {
        let front = self.tabs.get(self.active()).copied();
        self.tabs.retain(|tab| match tab {
            PanelTab::Project | PanelTab::New(_) => true,
            PanelTab::Surface(id) => alive(*id),
        });
        self.active = front
            .and_then(|tab| self.tabs.iter().position(|held| *held == tab))
            .unwrap_or_else(|| self.active.min(self.tabs.len().saturating_sub(1)));
    }

    /// One step along the row, `1` or `-1`, wrapping at either end — what
    /// the same chord already does to the window's project tabs. One chord,
    /// one behaviour, whichever row has the focus.
    pub fn step(&mut self, by: isize) {
        if self.tabs.len() < 2 {
            return;
        }
        let len = self.tabs.len() as isize;
        self.active = (self.active() as isize + by).rem_euclid(len) as usize;
    }

    /// Restore the drawer's shape from what was filed for this place. The
    /// tabs are handed in already resolved — a persisted tab whose record we
    /// could not find on disk is not passed here, so nothing is drawn from
    /// memory alone.
    pub fn restore(
        &mut self,
        open: bool,
        width: Option<f32>,
        surfaces: Vec<SurfaceId>,
        active: usize,
    ) {
        self.open = open;
        self.width = width;
        self.tabs = vec![PanelTab::Project];
        self.tabs
            .extend(surfaces.into_iter().map(PanelTab::Surface));
        self.active = active.min(self.tabs.len().saturating_sub(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u64) -> SurfaceId {
        SurfaceId(n)
    }

    #[test]
    fn starts_closed_on_the_project_tab() {
        let panel = Panel::default();
        assert!(!panel.open);
        assert_eq!(panel.tabs(), &[PanelTab::Project]);
        assert_eq!(panel.width(), SURFACE_WIDTH);
        assert_eq!(
            PAGE_WIDTH, SURFACE_WIDTH,
            "Project and Terminal share one default width"
        );
    }

    #[test]
    fn a_drag_remembers_the_width() {
        let mut panel = Panel::default();
        panel.set_width(640.0);
        assert_eq!(panel.width(), 640.0);
        panel.set_width(20.0);
        assert_eq!(panel.width(), MIN_WIDTH);
    }

    #[test]
    fn the_four_box_grows_the_drawer_instead_of_leaving_it() {
        let mut panel = Panel::default();
        panel.toggle_expanded_width(MAX_WIDTH);
        assert_eq!(panel.width(), MAX_WIDTH);
        panel.toggle_expanded_width(MAX_WIDTH);
        assert_eq!(panel.width(), SURFACE_WIDTH);
    }

    #[test]
    fn an_agents_surface_is_listed_but_does_not_open_the_drawer() {
        let mut panel = Panel::default();
        panel.add_surface(id(1), false, false);
        assert_eq!(panel.tabs().len(), 2);
        assert!(!panel.open, "the agent's own open must not open the drawer");
        assert_eq!(panel.active_tab(), PanelTab::Project);
    }

    #[test]
    fn the_persons_open_comes_to_the_front() {
        let mut panel = Panel::default();
        panel.add_surface(id(1), false, false);
        panel.add_surface(id(2), true, true);
        assert!(panel.open);
        assert_eq!(panel.active_tab(), PanelTab::Surface(id(2)));
        assert_eq!(panel.width(), SURFACE_WIDTH);
    }

    #[test]
    fn an_empty_tab_in_front_is_filled_rather_than_doubled() {
        let mut panel = Panel::default();
        panel.new_tab();
        assert_eq!(panel.tabs().len(), 2);
        panel.add_surface(id(7), true, true);
        assert_eq!(panel.tabs(), &[PanelTab::Project, PanelTab::Surface(id(7))]);
    }

    #[test]
    fn a_new_tab_lands_beside_the_one_in_front() {
        let mut panel = Panel::default();
        panel.add_surface(id(1), true, true);
        panel.add_surface(id(2), true, true);
        panel.select(1);
        panel.add_surface(id(3), true, true);
        assert_eq!(
            panel.tabs(),
            &[
                PanelTab::Project,
                PanelTab::Surface(id(1)),
                PanelTab::Surface(id(3)),
                PanelTab::Surface(id(2)),
            ],
            "beside the front tab, which is where Cursor opens one"
        );
        assert_eq!(panel.active(), 2);
    }

    #[test]
    fn stepping_wraps_as_the_project_tabs_do() {
        let mut panel = Panel::default();
        panel.add_surface(id(1), true, true);
        assert_eq!(panel.active(), 1);
        panel.step(1);
        assert_eq!(panel.active(), 0, "past the last comes round to the first");
        panel.step(-1);
        assert_eq!(panel.active(), 1, "and back the other way");
    }

    #[test]
    fn one_tab_has_nowhere_to_step() {
        let mut panel = Panel::default();
        panel.step(1);
        assert_eq!(panel.active(), 0);
    }

    #[test]
    fn the_project_tab_does_not_close() {
        let mut panel = Panel::default();
        panel.close(0);
        assert_eq!(panel.tabs(), &[PanelTab::Project]);
    }

    #[test]
    fn closing_the_front_tab_lands_beside_it() {
        let mut panel = Panel::default();
        panel.add_surface(id(1), true, true);
        panel.add_surface(id(2), true, true);
        panel.select(1);
        panel.close(1);
        assert_eq!(panel.tabs(), &[PanelTab::Project, PanelTab::Surface(id(2))]);
        assert_eq!(panel.active(), 1, "the tab that slid into its place");
    }

    #[test]
    fn a_surface_that_goes_takes_its_tab_with_it() {
        let mut panel = Panel::default();
        panel.add_surface(id(1), true, true);
        panel.add_surface(id(2), true, true);
        panel.retain_surfaces(|held| held == id(1));
        assert_eq!(panel.tabs(), &[PanelTab::Project, PanelTab::Surface(id(1))]);
        assert_eq!(panel.active(), 1);
    }

    #[test]
    fn the_tab_in_front_keeps_its_place_when_one_before_it_goes() {
        let mut panel = Panel::default();
        panel.add_surface(id(1), true, true);
        panel.add_surface(id(2), true, true);
        panel.add_surface(id(3), true, true);
        assert_eq!(panel.active_tab(), PanelTab::Surface(id(3)));
        panel.retain_surfaces(|held| held != id(1));
        assert_eq!(
            panel.active_tab(),
            PanelTab::Surface(id(3)),
            "the front tab is followed by identity, not by index"
        );
    }
}

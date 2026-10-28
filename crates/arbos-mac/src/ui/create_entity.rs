//! The one Create screen (D93: one style, parameterised by entity) for a new
//! agent, group or server — Grok Bot's "Create New Bot" layout: header with a
//! close ×, the preview, a centred name field, the picture options, and a
//! footer with Cancel + the action (disabled until named). Agents create from
//! here; groups go on to the existing member picker as step two ("Next");
//! servers keep their one-line explainer under the field.
//!
//! **Picture (Jacob, 2026-10-25, [feedback create-sheet-avatar-upload-generate-mac-2026-10-25]):
//! the sixteen-leaf grid and the ten pastel swatches are gone. Under the name field sit the
//! agent avatar picker's own two options — `Upload | Generate` (`avatar_picker::tab_pill`,
//! `upload_body`, `generate_body` on a draft [`avatar_picker::Picker`]) — and the preview
//! above the field shows the chosen file, the generated picture, or, when neither was picked,
//! the fallback every new agent / group / server gets: the name's first letter on a neutral
//! grey disc (`avatars::neutral_element`). The entity is stored with the neutral pair
//! (`leaf::NEUTRAL`), never an automatic leaf; existing agents keep theirs.**
//!
//! Header, preview and field keep Grok's sizes, measured on the 2x reference
//! (`media/leaf-avatars/create-sheet/ref-grok-create-new-bot.png`) and written here in
//! points; the option rows start where Grok's first icon row did.

use gpui::{div, prelude::*, px, Context, Div, FontWeight, Hsla, MouseButton};

use crate::ui::avatar_picker::{self, Picker, Tab};
use crate::ui::avatars;
use crate::ui::frost::{frosted, PANEL_BLUR};
use crate::ui::highlight;
use crate::ui::icons::icon;
use crate::ui::shell::Shell;
use crate::ui::theme::{Appearance, Theme};

pub(crate) const WIDTH: f32 = 440.0;
const RADIUS: f32 = 16.0;
const HEADER: f32 = 54.5;
pub(crate) const PREVIEW: f32 = 114.0;
const PREVIEW_TOP: f32 = 62.0;
const FIELD_TOP: f32 = 183.0;
const FIELD_H: f32 = 35.0;
const FIELD_INSET: f32 = 16.5;
/// The `Upload | Generate` capsules: a 28 pt row whose top is where Grok's first icon row
/// began (its 36 pt rings centred on 264).
pub(crate) const OPTIONS_TOP: f32 = 246.0;
const OPTIONS_H: f32 = 28.0;
/// The chosen option's body starts 16 under the capsules.
pub(crate) const BODY_TOP: f32 = OPTIONS_TOP + OPTIONS_H + 16.0;
/// Upload: the "Choose a photo…" pill (28) then its hint 12 under.
const UPLOAD_HINT_TOP: f32 = BODY_TOP + 28.0 + 12.0;
/// Generate: the prompt field (35), the Generate pill 12 under (28), the status 12 under that.
const GENERATE_STATUS_TOP: f32 = BODY_TOP + 35.0 + 12.0 + 28.0 + 12.0;
/// The footer sits 12 under the Generate status line's 16 pt box, the taller of the two bodies,
/// so the sheet does not jump when the option changes.
pub(crate) const FOOTER_TOP: f32 = GENERATE_STATUS_TOP + 16.0 + 12.0;
const FOOTER_H: f32 = 56.5;
const PILL_H: f32 = 27.0;
pub(crate) const HEIGHT: f32 = FOOTER_TOP + FOOTER_H;
/// The server explainer (13 pt, two lines at this width) plus its gap: everything under the
/// field moves down by this much.
const EXPLAINER_SHIFT: f32 = 40.0;
/// Connect an agent (#2141): the transport capsules under the explainer (28 pt row + gap).
const TRANSPORT_SHIFT: f32 = 40.0;

/// What the sheet creates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Agent,
    Group,
    Server,
    /// Connect an outside agent (#2141): same screen, then the key reveal instead of a box.
    Connect,
}

/// Where a connected agent runs — the three rows of the approved type picker, as capsules.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Transport {
    /// Claude Code / Codex on this Mac over the Agent Client Protocol (`arbos-acp`).
    #[default]
    Acp,
    /// A Grok Bot assistant: a routine calls the relay over HTTPS.
    Grok,
    /// Any bot that speaks HTTPS or a WebSocket with the key.
    Custom,
}

impl Transport {
    pub const ALL: [Transport; 3] = [Transport::Acp, Transport::Grok, Transport::Custom];

    pub fn label(self) -> &'static str {
        match self {
            Transport::Acp => "Claude Code / Codex on this Mac",
            Transport::Grok => "Grok",
            Transport::Custom => "Custom bot",
        }
    }

    /// The relay's word.
    pub fn wire(self) -> &'static str {
        match self {
            Transport::Acp => "acp",
            Transport::Grok => "grok",
            Transport::Custom => "custom",
        }
    }

    pub fn from_wire(s: &str) -> Transport {
        match s {
            "acp" => Transport::Acp,
            "grok" => Transport::Grok,
            _ => Transport::Custom,
        }
    }

    /// The one line under the key that says what to do with it.
    pub fn how_to(self) -> &'static str {
        match self {
            Transport::Acp => "In Terminal: cargo install --git https://github.com/unarbos/arbos arbos-acp, then run arbos-acp with the key — it starts Claude Code or Codex for you.",
            Transport::Grok => "In Grok, add a routine that calls the link every few minutes with the key. Arbos handles the rest.",
            Transport::Custom => "Your bot calls the link with the key: read its inbox, post replies. The link opens the short how-to.",
        }
    }
}

impl Kind {
    fn title(self) -> &'static str {
        match self {
            Kind::Agent => "Create New Agent",
            Kind::Group => "Create New Group",
            Kind::Server => "Create New Server",
            Kind::Connect => "Connect an Agent",
        }
    }

    pub fn placeholder(self) -> &'static str {
        match self {
            Kind::Agent | Kind::Connect => "Name your agent",
            Kind::Group => "Name your group",
            Kind::Server => "Name your server",
        }
    }

    /// The footer action: groups continue to the member picker.
    fn action(self) -> &'static str {
        match self {
            Kind::Agent | Kind::Server | Kind::Connect => "Create",
            Kind::Group => "Next",
        }
    }

    fn explainer(self) -> Option<&'static str> {
        match self {
            Kind::Server => Some(
                "A server has channels and an invite link. You start as its owner, in #general.",
            ),
            Kind::Connect => Some(
                "An agent you already run joins your chats as a member. You get a key to give it.",
            ),
            Kind::Agent | Kind::Group => None,
        }
    }

    fn shift(self) -> f32 {
        let mut shift = 0.0;
        if self.explainer().is_some() {
            shift += EXPLAINER_SHIFT;
        }
        if self == Kind::Connect {
            shift += TRANSPORT_SHIFT;
        }
        shift
    }
}

/// Grok's dark sheet palette (approved as is); the light theme keeps its own.
pub(crate) struct Palette {
    pub(crate) bg: Hsla,
    pub(crate) line: Hsla,
    pub(crate) title: Hsla,
    pub(crate) placeholder: Hsla,
    pub(crate) ring: Hsla,
    pub(crate) cancel_bg: Hsla,
    pub(crate) create_bg: Hsla,
    pub(crate) create_text: Hsla,
}

pub(crate) fn hex(v: u32) -> Hsla {
    gpui::Rgba {
        r: ((v >> 16) & 0xff) as f32 / 255.0,
        g: ((v >> 8) & 0xff) as f32 / 255.0,
        b: (v & 0xff) as f32 / 255.0,
        a: 1.0,
    }
    .into()
}

pub(crate) fn palette(theme: &Theme) -> Palette {
    match theme.appearance {
        Appearance::Dark => Palette {
            bg: hex(0x1F1F1F),
            line: hex(0x353535),
            title: hex(0xF5F5F5),
            placeholder: hex(0x939393),
            ring: hex(0x505050),
            cancel_bg: hex(0x272727),
            create_bg: hex(0x3B3B3B),
            create_text: hex(0x737373),
        },
        Appearance::Light => Palette {
            bg: theme.card(),
            line: theme.border,
            title: theme.text,
            placeholder: theme.text_muted,
            ring: theme.border_strong,
            cancel_bg: theme.field,
            create_bg: theme.field,
            create_text: theme.text_faint,
        },
    }
}

/// The two options every Create sheet offers, in order — the draft picker's tabs.
pub fn options() -> Vec<Tab> {
    Tab::for_target(&avatar_picker::Target::Draft)
}

/// The leaf pair a new entity is stored with: the neutral one, whatever else was picked, so
/// no leaf is ever auto-assigned (a later Reset lands on the initial, not on a leaf).
pub fn stored_leaf() -> avatars::LeafPick {
    avatars::neutral_pick()
}

/// The preview above the field: the pending file or generated picture, else the neutral
/// initial of the typed name (an empty disc until something is typed).
pub(crate) fn preview(picker: &Picker, name: &str, shell: &Shell, cx: &Context<Shell>) -> Div {
    let theme = Theme::of(cx);
    let state = shell.state.read(cx);
    avatar_picker::pending_preview(picker, state, PREVIEW)
        .unwrap_or_else(|| avatars::neutral_element(name, PREVIEW, theme))
}

pub fn sheet(shell: &Shell, kind: Kind, cx: &mut Context<Shell>) -> Div {
    let theme = Theme::of(cx).clone();
    let p = palette(&theme);
    let picker = shell.create_picker.clone();
    let input = match kind {
        Kind::Agent | Kind::Group | Kind::Connect => shell.project_input.clone(),
        Kind::Server => shell.server_input.clone(),
    };
    let typed = input.read(cx).text().trim().to_string();
    let busy = match kind {
        Kind::Server => shell.state.read(cx).server_busy,
        Kind::Connect => shell.state.read(cx).awaiting_new_project,
        Kind::Agent | Kind::Group => false,
    };
    let can_go = !typed.is_empty() && !busy && !picker.generating;
    let shift = kind.shift();

    let header = div()
        .relative()
        .flex_none()
        .w_full()
        .h(px(HEADER))
        .border_b_1()
        .border_color(p.line)
        .child(
            div()
                .absolute()
                .left(px(17.5))
                .top(px(16.0))
                .text_size(px(14.2))
                .line_height(px(20.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(p.title)
                .child(kind.title()),
        )
        .child(
            div()
                .id("create-entity-close")
                .absolute()
                .right(px(16.5))
                .top(px(15.0))
                .size(px(22.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .cursor_pointer()
                .map(|el| highlight::button(el, &theme))
                .child(icon("xmark", 10.0, hex(0xD2D2D2)))
                .on_click(
                    cx.listener(move |this, _, window, cx| this.cancel_create(kind, window, cx)),
                ),
        );

    let preview = div()
        .id("create-entity-preview")
        .debug_selector(|| "create-entity-preview".into())
        .absolute()
        .left_0()
        .right_0()
        .top(px(PREVIEW_TOP - HEADER))
        .flex()
        .justify_center()
        .child(preview(&picker, &typed, shell, cx));

    let field = div()
        .absolute()
        .left(px(FIELD_INSET))
        .right(px(FIELD_INSET))
        .top(px(FIELD_TOP - HEADER))
        .h(px(FIELD_H))
        .rounded(px(8.0))
        .border_1()
        .border_color(p.line)
        .flex()
        .items_center()
        .px(px(12.0))
        .text_size(px(15.5))
        .text_color(if typed.is_empty() {
            p.placeholder
        } else {
            p.title
        })
        .child(input);
    let explainer = kind.explainer().map(|text| {
        div()
            .absolute()
            .left(px(FIELD_INSET))
            .right(px(FIELD_INSET))
            .top(px(FIELD_TOP + FIELD_H + 8.0 - HEADER))
            .text_size(px(13.0))
            .line_height(px(16.0))
            .text_center()
            .text_color(p.placeholder)
            .child(text)
    });
    // Connect an agent: the transport capsules (the approved type picker's three rows) under
    // the explainer, where the explainer's gap ends.
    let transport_row = (kind == Kind::Connect).then(|| {
        let selected = shell.connect_transport;
        let label_muted = match theme.appearance {
            Appearance::Dark => hex(0x9A9A9A),
            Appearance::Light => theme.text_muted,
        };
        let mut row = div().h(px(OPTIONS_H)).flex().items_center().gap(px(2.0));
        for t in Transport::ALL {
            let on = t == selected;
            row = row.child(
                div()
                    .id(gpui::ElementId::Name(gpui::SharedString::from(format!(
                        "connect-transport-{}",
                        t.wire()
                    ))))
                    .debug_selector(move || format!("connect-transport-{}", t.wire()))
                    .h(px(OPTIONS_H))
                    .px(px(11.0))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .text_size(px(13.0))
                    .font_weight(FontWeight::MEDIUM)
                    .when(on, |d| d.bg(p.cancel_bg).text_color(p.title))
                    .when(!on, |d| d.text_color(label_muted))
                    .cursor_pointer()
                    .hover(|s| s.opacity(0.85))
                    .child(t.label())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.connect_transport = t;
                        cx.notify();
                    })),
            );
        }
        div()
            .id("connect-transports")
            .debug_selector(|| "connect-transports".into())
            .absolute()
            .left_0()
            .right_0()
            .top(px(FIELD_TOP + FIELD_H + 8.0 + EXPLAINER_SHIFT - HEADER))
            .flex()
            .justify_center()
            .child(row)
    });

    // ----- the two options: Upload | Generate, then the chosen one's body ------------------
    let options = div()
        .id("create-entity-options")
        .debug_selector(|| "create-entity-options".into())
        .absolute()
        .left_0()
        .right_0()
        .top(px(OPTIONS_TOP + shift - HEADER))
        .flex()
        .justify_center()
        .child(avatar_picker::tab_pill(
            &options(),
            picker.tab,
            &p,
            &theme,
            cx,
        ));
    let body: Vec<gpui::AnyElement> = match picker.tab {
        Tab::Upload => avatar_picker::upload_body(
            &picker,
            avatar_picker::Slots {
                top: BODY_TOP + shift - HEADER,
                status_top: UPLOAD_HINT_TOP + shift - HEADER,
            },
            None,
            &p,
            cx,
        ),
        Tab::Generate => avatar_picker::generate_body(
            shell,
            &picker,
            avatar_picker::Slots {
                top: BODY_TOP + shift - HEADER,
                status_top: GENERATE_STATUS_TOP + shift - HEADER,
            },
            None,
            &p,
            &theme,
            cx,
        ),
        // Never offered on a draft (`options()`); nothing to draw.
        Tab::Bot => Vec::new(),
    };

    let pill = |id: &'static str, label: &'static str, w: f32, bg: Hsla, fg: Hsla| {
        div()
            .id(id)
            .absolute()
            .top(px(15.0))
            .w(px(w))
            .h(px(PILL_H))
            .rounded_full()
            .bg(bg)
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(14.2))
            .font_weight(FontWeight::MEDIUM)
            .text_color(fg)
            .child(label)
    };
    let footer = div()
        .id("create-entity-footer")
        .debug_selector(|| "create-entity-footer".into())
        .absolute()
        .left_0()
        .right_0()
        .top(px(FOOTER_TOP + shift - HEADER))
        .h(px(FOOTER_H))
        .border_t_1()
        .border_color(p.line)
        .child(
            pill("create-entity-cancel", "Cancel", 68.5, p.cancel_bg, p.title)
                .right(px(12.5 + 67.0 + 8.0))
                .cursor_pointer()
                .hover(|s| s.opacity(0.85))
                .on_click(
                    cx.listener(move |this, _, window, cx| this.cancel_create(kind, window, cx)),
                ),
        )
        .child(if can_go {
            pill(
                "create-entity-go",
                kind.action(),
                67.0,
                theme.primary,
                theme.on_primary,
            )
            .right(px(12.5))
            .cursor_pointer()
            .hover(|s| s.opacity(0.88))
            .on_click(cx.listener(move |this, _, window, cx| this.submit_create(kind, window, cx)))
        } else {
            pill(
                "create-entity-go",
                if busy { "Creating…" } else { kind.action() },
                67.0,
                p.create_bg,
                p.create_text,
            )
            .right(px(12.5))
        });

    // A column: header · the note (a failed generation, Jacob 2026-10-26) · the body's fixed
    // box with everything else in its absolute slots · (the footer is the box's last slot).
    // Without a note the card is exactly `HEIGHT + shift`; a note grows it by itself.
    let note = avatar_picker::error_slot(&picker, &theme, cx);
    let card = div()
        .id("create-entity-card")
        .debug_selector(|| "create-entity-card".into())
        .relative()
        .w(px(WIDTH))
        .max_w_full()
        .max_h_full()
        .flex()
        .flex_col()
        .rounded(px(RADIUS))
        .bg(p.bg)
        .border_1()
        .border_color(p.line)
        .shadow_lg()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(header)
        .children(note)
        .child(
            div()
                .relative()
                .flex_none()
                .w_full()
                .h(px(HEIGHT + shift - HEADER))
                .child(preview)
                .child(field)
                .children(explainer)
                .children(transport_row)
                .child(options)
                .children(body)
                .child(footer),
        );
    // The same modal backdrop as the New Message sheet: a click outside closes.
    div()
        .absolute()
        .inset_0()
        .occlude()
        .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
        .flex()
        .items_center()
        .justify_center()
        .p(px(24.0))
        .bg(Theme::with_alpha(theme.bg, 0.55))
        .child(frosted(RADIUS, PANEL_BLUR, card))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, window, cx| this.cancel_create(kind, window, cx)),
        )
}

#[cfg(test)]
mod tests {
    //! Jacob, 2026-10-25: the Create sheets offer Upload and Generate only, and an entity
    //! created without a pick is stored neutral (the name's initial on a grey disc), never
    //! with an automatic leaf.

    use super::*;

    #[test]
    fn the_sheet_offers_upload_and_generate_only() {
        assert_eq!(options(), [Tab::Upload, Tab::Generate]);
        assert!(!options().contains(&Tab::Bot), "no leaf grid, no swatches");
    }

    #[test]
    fn a_new_entity_is_stored_neutral_never_with_an_automatic_leaf() {
        let stored = stored_leaf();
        assert!(avatars::leaf::is_neutral(Some(stored.leaf)));
        assert_eq!(stored.color, avatars::leaf::NEUTRAL);
        // The automatic pair an id would get is not what the sheet sends.
        for seed in ["new-agent-0", "new-group-3", "new-server-1"] {
            assert_ne!(avatars::leaf::default_pick(seed), stored);
        }
    }

    #[test]
    fn the_option_rows_sit_where_the_leaf_grid_was_and_the_sheet_got_shorter() {
        // Grok's first icon row: 36 pt rings centred on 264 → top 246. The rows read top to
        // bottom in order, and the leaf sheet's footer sat at 546 (602.5 tall): without the
        // grid the card is shorter.
        let rows = std::hint::black_box([
            OPTIONS_TOP,
            OPTIONS_TOP + OPTIONS_H,
            BODY_TOP,
            UPLOAD_HINT_TOP,
            GENERATE_STATUS_TOP,
            GENERATE_STATUS_TOP + 16.0,
            FOOTER_TOP,
            HEIGHT,
        ]);
        assert_eq!(rows[0], 246.0);
        assert!(rows.windows(2).all(|w| w[0] < w[1]), "{rows:?}");
        assert!(rows[7] < 546.0, "{rows:?}");
    }
}

#[cfg(test)]
mod render_tests {
    //! Headless render of the Create New Agent and Create New Group sheets (the shell's own
    //! `create_entity::sheet` on an idle app state): no leaf grid, no swatch, the two options,
    //! and the neutral initial in the preview when nothing was picked.

    use futures::future::BoxFuture;
    use gpui::{Entity, TestAppContext, VisualTestContext};
    use tokio::sync::mpsc;

    use super::*;
    use crate::app::AppState;
    use crate::sync::{ClientCommand, SyncEvent, SyncSource};
    use crate::ui::motion::PulseClock;

    struct Idle;

    impl SyncSource for Idle {
        fn name(&self) -> &'static str {
            "idle"
        }

        fn run(
            self: Box<Self>,
            _events: mpsc::UnboundedSender<SyncEvent>,
            _commands: mpsc::UnboundedReceiver<ClientCommand>,
        ) -> BoxFuture<'static, anyhow::Result<()>> {
            Box::pin(std::future::pending())
        }
    }

    fn mount(
        cx: &mut TestAppContext,
    ) -> (
        tokio::runtime::Runtime,
        Entity<AppState>,
        Entity<Shell>,
        &mut VisualTestContext,
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio");
        let handle = rt.handle().clone();
        cx.update(|cx| {
            gpui_tokio::init_from_handle(cx, handle);
            cx.set_global(Theme::dark(crate::ui::theme::Surface::Matte));
            if !cx.has_global::<PulseClock>() {
                PulseClock::init(cx);
            }
        });
        let state = cx.new(|cx| AppState::new(Box::new(Idle), cx));
        let (shell, vcx) = cx.add_window_view(|window, cx| Shell::new(state.clone(), window, cx));
        vcx.run_until_parked();
        (rt, state, shell, vcx)
    }

    fn settle(shell: &Entity<Shell>, vcx: &mut VisualTestContext) {
        for _ in 0..3 {
            shell.update(vcx, |_, cx| cx.notify());
            vcx.run_until_parked();
        }
    }

    /// Open the Create sheet for `kind` with `name` typed, as the New Message sheet's rows do.
    fn open(
        kind: Kind,
        name: &str,
        state: &Entity<AppState>,
        shell: &Entity<Shell>,
        vcx: &mut VisualTestContext,
    ) {
        state.update(vcx, |s, cx| s.open_new_chat(cx));
        match kind {
            Kind::Agent => {
                shell.update(vcx, |this, cx| this.open_create_agent(cx));
                state.update(vcx, |s, cx| {
                    s.new_project_open = true;
                    cx.notify();
                });
            }
            Kind::Group => shell.update_in(vcx, |this, window, cx| {
                this.open_create_group(window, cx);
            }),
            Kind::Connect => shell.update_in(vcx, |this, window, cx| {
                this.open_connect_agent(window, cx);
            }),
            Kind::Server => state.update(vcx, |s, cx| {
                s.open_server_sheet(crate::app::ServerSheet::NewServer, cx)
            }),
        }
        let name = name.to_owned();
        shell.update(vcx, |this, cx| {
            let input = match kind {
                Kind::Agent | Kind::Group | Kind::Connect => this.project_input.clone(),
                Kind::Server => this.server_input.clone(),
            };
            input.update(cx, |input, cx| input.set_text(name, cx));
        });
        settle(shell, vcx);
    }

    fn drawn(vcx: &mut VisualTestContext, selector: &'static str) -> bool {
        vcx.debug_bounds(selector).is_some()
    }

    #[gpui::test]
    fn both_create_sheets_show_two_options_and_no_leaf_grid(cx: &mut TestAppContext) {
        let (_rt, state, shell, vcx) = mount(cx);
        for kind in [Kind::Agent, Kind::Group] {
            open(kind, "Field notes", &state, &shell, vcx);
            assert!(
                drawn(vcx, "create-entity-card"),
                "{kind:?}: the sheet is up"
            );
            assert!(
                drawn(vcx, "create-entity-options"),
                "{kind:?}: the option capsules are drawn"
            );
            assert!(
                drawn(vcx, "avatar-tab-upload") && drawn(vcx, "avatar-tab-generate"),
                "{kind:?}: Upload | Generate"
            );
            assert!(!drawn(vcx, "avatar-tab-bot"), "{kind:?}: no Bot tab");
            assert!(
                !drawn(vcx, "leaf-hover") && !drawn(vcx, "leaf-hero"),
                "{kind:?}: no leaf anywhere on the sheet (no grid, no leaf preview)"
            );
            assert!(
                drawn(vcx, "avatar-upload-choose"),
                "{kind:?}: Upload is the first option"
            );
            let card = vcx.debug_bounds("create-entity-card").expect("card");
            let footer = vcx.debug_bounds("create-entity-footer").expect("footer");
            let preview = vcx.debug_bounds("create-entity-preview").expect("preview");
            // Children are placed inside the card's 1 pt border.
            let footer_top: f32 = (footer.origin.y - card.origin.y).into();
            assert!(
                (footer_top - FOOTER_TOP - 1.0).abs() < 0.5,
                "{kind:?}: footer at {footer_top}, expected {FOOTER_TOP} + the border"
            );
            let card_h: f32 = card.size.height.into();
            assert!(
                (card_h - HEIGHT).abs() < 2.5,
                "{kind:?}: card {card_h} tall"
            );
            assert!(f32::from(preview.size.height) >= PREVIEW - 0.5);
            // Nothing picked → the create carries no picture and the neutral pair.
            let picked = shell.read_with(vcx, |this, _| this.create_picker.draft_picture());
            assert_eq!(picked, None, "{kind:?}");
            shell.update_in(vcx, |this, window, cx| this.cancel_create(kind, window, cx));
            settle(&shell, vcx);
            assert!(!drawn(vcx, "create-entity-card"), "{kind:?}: closed");
        }
    }

    /// Connect an agent (#2141): the same Create screen with the three transport capsules under
    /// the explainer, the ACP one selected first; Cancel closes it.
    #[gpui::test]
    fn the_connect_sheet_shows_the_transport_capsules(cx: &mut TestAppContext) {
        let (_rt, state, shell, vcx) = mount(cx);
        open(Kind::Connect, "Grok Bot", &state, &shell, vcx);
        assert!(drawn(vcx, "create-entity-card"), "the sheet is up");
        assert!(
            drawn(vcx, "connect-transports"),
            "the transport row is drawn"
        );
        for id in [
            "connect-transport-acp",
            "connect-transport-grok",
            "connect-transport-custom",
        ] {
            assert!(drawn(vcx, id), "{id} capsule");
        }
        assert_eq!(
            shell.read_with(vcx, |this, _| this.connect_transport),
            Transport::Acp
        );
        shell.update(vcx, |this, cx| {
            this.connect_transport = Transport::Grok;
            cx.notify();
        });
        settle(&shell, vcx);
        assert_eq!(
            shell.read_with(vcx, |this, _| this.connect_transport),
            Transport::Grok
        );
        shell.update_in(vcx, |this, window, cx| {
            this.cancel_create(Kind::Connect, window, cx)
        });
        settle(&shell, vcx);
        assert!(!drawn(vcx, "create-entity-card"), "closed");
        assert!(!state.read_with(vcx, |s, _| s.connect_open));
    }

    /// With neither option used, the preview is the neutral initial — no leaf element at all
    /// on the sheet — and switching to Generate shows the prompt body, still no leaf.
    #[gpui::test]
    fn the_preview_falls_back_to_the_neutral_initial(cx: &mut TestAppContext) {
        let (_rt, state, shell, vcx) = mount(cx);
        open(Kind::Agent, "Field notes", &state, &shell, vcx);
        assert!(drawn(vcx, "create-entity-preview"));
        assert!(
            vcx.debug_bounds("leaf-hero").is_none() && vcx.debug_bounds("leaf-hover").is_none(),
            "the neutral preview draws no leaf"
        );
        let has_pending = shell.read_with(vcx, |this, cx| {
            avatar_picker::pending_preview(&this.create_picker, this.state.read(cx), PREVIEW)
                .is_some()
        });
        assert!(!has_pending);
        assert_eq!(avatars::leaf::initial("Field notes"), "F");
        shell.update(vcx, |this, cx| {
            this.create_picker.tab = Tab::Generate;
            cx.notify();
        });
        settle(&shell, vcx);
        assert!(drawn(vcx, "avatar-generate-go"), "the Generate body is up");
        assert!(drawn(vcx, "avatar-generate-status"));
        assert!(!drawn(vcx, "avatar-upload-choose"));
        assert!(vcx.debug_bounds("leaf-hero").is_none());
    }
}

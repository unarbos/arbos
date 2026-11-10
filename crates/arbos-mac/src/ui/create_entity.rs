//! The one Create screen (D93: one style, parameterised by entity) for a new
//! agent, group or server — Grok Bot's **Edit-page hierarchy** as a modal sheet
//! ([feedback iphone-create-entity-edit-style-2026-11-10]): header with close ×
//! and a Create/Next Done pill, centred avatar with a pencil affordance, then
//! section headers + grouped cards (Name, Appearance with Upload | Generate,
//! Connection for connect / an explainer for server). Agents create from here;
//! groups go on to the existing member picker as step two ("Next"); servers keep
//! their one-line explainer under the name field. Mac keeps the modal sheet
//! idiom (not an iOS push).
//!
//! **Picture (Jacob, 2026-10-25, [feedback create-sheet-avatar-upload-generate-mac-2026-10-25]):
//! the sixteen-shape grid and the ten pastel swatches are gone. Appearance offers
//! `Upload | Generate` (`avatar_picker::tab_pill`, `upload_body`, `generate_body`
//! on a draft [`avatar_picker::Picker`]) — and the preview shows the chosen file,
//! the generated picture, or, when neither was picked, the sheet's own placeholder:
//! the name's first letter on a grey disc (`avatars::neutral_element`). That grey
//! is sheet chrome, not a default avatar — the entity has no id yet, so no colour
//! can be derived. The entity is stored with no colour at all (`stored_disc()` →
//! `None`), and the disc follows from the id the relay assigns.**

use gpui::{
    div, prelude::*, px, Context, Div, ElementId, FontWeight, Hsla, MouseButton, SharedString,
};

use crate::ui::avatar_picker::{self, Picker, Tab};
use crate::ui::avatars;
use crate::ui::frost::{frosted, PANEL_BLUR};
use crate::ui::highlight;
use crate::ui::profiles::kit;
use crate::ui::sf_symbols;
use crate::ui::shell::Shell;
use crate::ui::theme::{Appearance, Theme, TEXT_TINY};

pub(crate) const WIDTH: f32 = 440.0;
const RADIUS: f32 = 16.0;
const HEADER: f32 = 54.5;
pub(crate) const PREVIEW: f32 = 114.0;
const FIELD_H: f32 = 35.0;
const FIELD_INSET: f32 = 16.5;
const OPTIONS_H: f32 = 28.0;
/// Upload body height inside the Appearance card (pill + hint).
const UPLOAD_BODY_H: f32 = 28.0 + 12.0 + 16.0;
/// Generate body height (prompt + pill + status).
const GENERATE_BODY_H: f32 = 35.0 + 12.0 + 28.0 + 12.0 + 16.0;
/// Kept for tests / callers that measured the old absolute layout: the option
/// capsules once sat where Grok's first icon row began.
pub(crate) const OPTIONS_TOP: f32 = 246.0;
pub(crate) const BODY_TOP: f32 = OPTIONS_TOP + OPTIONS_H + 16.0;
pub(crate) const FOOTER_TOP: f32 = BODY_TOP + GENERATE_BODY_H + 12.0;
pub(crate) const HEIGHT: f32 = FOOTER_TOP + 56.5;

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

/// The disc colour a new entity is stored with: **none**, so the colour is derived from the id
/// the relay assigns. A new entity has no id yet, so no colour can be chosen here, and storing a
/// sentinel would be storing "draw me grey" — which no default does any more.
pub fn stored_disc() -> Option<u8> {
    None
}

/// The preview above the field: the pending file or generated picture, else the neutral
/// initial of the typed name (an empty disc until something is typed).
pub(crate) fn preview(picker: &Picker, name: &str, shell: &Shell, cx: &Context<Shell>) -> Div {
    let theme = Theme::of(cx);
    let state = shell.state.read(cx);
    avatar_picker::pending_preview(picker, state, PREVIEW)
        .unwrap_or_else(|| avatars::neutral_element(name, PREVIEW, theme))
}

/// Pencil badge on the create / edit hero avatar (Grok Edit Bot).
fn pencil_badge(theme: &Theme) -> Div {
    div()
        .absolute()
        .right(px(2.0))
        .bottom(px(2.0))
        .size(px(28.0))
        .rounded_full()
        .bg(theme.control_fill())
        .border_1()
        .border_color(theme.border)
        .flex()
        .items_center()
        .justify_center()
        .child(sf_symbols::glyph("pencil", 12.0, theme.text))
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
    let option_label = match picker.tab {
        Tab::Upload => "Upload",
        Tab::Generate => "Generate",
        Tab::Bot => "Bot",
    };

    // ----- header: × · title · Create/Next Done pill ---------------------------------
    let header = div()
        .relative()
        .flex_none()
        .w_full()
        .h(px(HEADER))
        .border_b_1()
        .border_color(p.line)
        .flex()
        .items_center()
        .px(px(16.0))
        .child(
            div()
                .id("create-entity-close")
                .size(px(22.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .cursor_pointer()
                .map(|el| highlight::button(el, &theme))
                .child(sf_symbols::glyph("xmark", 10.0, hex(0xD2D2D2)))
                .on_click(
                    cx.listener(move |this, _, window, cx| this.cancel_create(kind, window, cx)),
                ),
        )
        .child(
            div()
                .flex_1()
                .px(px(12.0))
                .text_size(px(14.2))
                .line_height(px(20.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(p.title)
                .text_center()
                .child(kind.title()),
        )
        .child({
            let label = if busy { "Creating…" } else { kind.action() };
            let pill = kit::done_pill("create-entity-go", label, can_go, &theme);
            if can_go {
                pill.on_click(
                    cx.listener(move |this, _, window, cx| this.submit_create(kind, window, cx)),
                )
            } else {
                pill
            }
        });

    // ----- hero avatar with pencil ---------------------------------------------------
    let hero = div()
        .id("create-entity-preview")
        .debug_selector(|| "create-entity-preview".into())
        .w_full()
        .flex()
        .justify_center()
        .pt(px(18.0))
        .pb(px(8.0))
        .child(
            div()
                .relative()
                .size(px(PREVIEW))
                .child(preview(&picker, &typed, shell, cx))
                .child(pencil_badge(&theme)),
        );

    // ----- Name section --------------------------------------------------------------
    let name_section = div()
        .w_full()
        .flex()
        .flex_col()
        .child(kit::edit_section_header("Name", &theme))
        .child(
            kit::card(&theme).child(
                div()
                    .id("create-entity-name")
                    .debug_selector(|| "create-entity-name".into())
                    .h(px(FIELD_H + 6.0))
                    .px(px(FIELD_INSET))
                    .flex()
                    .items_center()
                    .text_size(px(15.5))
                    .text_color(if typed.is_empty() {
                        p.placeholder
                    } else {
                        p.title
                    })
                    .child(input),
            ),
        )
        .children(kind.explainer().map(|text| {
            div()
                .px(px(kit::INSET + kit::SECTION_TITLE_INDENT))
                .mt(px(-6.0))
                .mb(px(8.0))
                .text_size(px(TEXT_TINY))
                .line_height(px(16.0))
                .text_color(p.placeholder)
                .child(text)
        }));

    // ----- Appearance: Avatar row + Upload | Generate --------------------------------
    let body_h = match picker.tab {
        Tab::Generate => GENERATE_BODY_H,
        Tab::Upload | Tab::Bot => UPLOAD_BODY_H,
    };
    let appearance_inner_h = OPTIONS_H + 16.0 + body_h;
    let appearance_body: Vec<gpui::AnyElement> = match picker.tab {
        Tab::Upload => avatar_picker::upload_body(
            &picker,
            avatar_picker::Slots {
                top: OPTIONS_H + 16.0,
                status_top: OPTIONS_H + 16.0 + 28.0 + 12.0,
            },
            None,
            &p,
            cx,
        ),
        Tab::Generate => avatar_picker::generate_body(
            shell,
            &picker,
            avatar_picker::Slots {
                top: OPTIONS_H + 16.0,
                status_top: OPTIONS_H + 16.0 + 35.0 + 12.0 + 28.0 + 12.0,
            },
            None,
            &p,
            &theme,
            cx,
        ),
        Tab::Bot => Vec::new(),
    };
    let appearance_section = div()
        .w_full()
        .flex()
        .flex_col()
        .child(kit::edit_section_header("Appearance", &theme))
        .child(
            kit::card(&theme)
                .child(
                    kit::row(
                        "create-avatar-row",
                        kit::Leading::None,
                        "Avatar",
                        None,
                        Some(option_label.into()),
                        None,
                        true,
                        &theme,
                    )
                    .into_any_element(),
                )
                .child(
                    div()
                        .id("create-entity-options")
                        .debug_selector(|| "create-entity-options".into())
                        .relative()
                        .w_full()
                        .h(px(appearance_inner_h))
                        .child(
                            div()
                                .absolute()
                                .left_0()
                                .right_0()
                                .top(px(0.0))
                                .h(px(OPTIONS_H))
                                .flex()
                                .justify_center()
                                .child(avatar_picker::tab_pill(
                                    &options(),
                                    picker.tab,
                                    &p,
                                    &theme,
                                    cx,
                                )),
                        )
                        .children(appearance_body),
                ),
        );

    // ----- Connection (connect an agent) ---------------------------------------------
    let connection = (kind == Kind::Connect).then(|| {
        let selected = shell.connect_transport;
        let mut rows: Vec<gpui::AnyElement> = Vec::new();
        for t in Transport::ALL {
            let on = t == selected;
            let wire = t.wire();
            let check =
                on.then(|| sf_symbols::glyph("checkmark", 14.0, p.title).into_any_element());
            let id = format!("connect-transport-{wire}");
            rows.push(
                kit::row(
                    ElementId::Name(SharedString::from(id.clone())),
                    kit::Leading::None,
                    t.label(),
                    None,
                    None,
                    check,
                    false,
                    &theme,
                )
                .debug_selector(move || id.clone())
                .cursor_pointer()
                .group(kit::HOT)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.connect_transport = t;
                    cx.notify();
                }))
                .into_any_element(),
            );
        }
        div()
            .id("connect-transports")
            .debug_selector(|| "connect-transports".into())
            .w_full()
            .flex()
            .flex_col()
            .child(kit::edit_section_header("Connection", &theme))
            .child(kit::section(rows, &theme))
    });

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
                .id("create-entity-body")
                .debug_selector(|| "create-entity-body".into())
                .flex()
                .flex_col()
                .flex_1()
                .w_full()
                .overflow_y_scroll()
                .pb(px(24.0))
                .child(hero)
                .child(name_section)
                .child(appearance_section)
                .children(connection),
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
    //! with an automatic colour.

    use super::*;

    #[test]
    fn the_sheet_offers_upload_and_generate_only() {
        assert_eq!(options(), [Tab::Upload, Tab::Generate]);
        assert!(!options().contains(&Tab::Bot), "no swatches");
    }

    #[test]
    fn a_new_entity_stores_no_colour_so_its_id_picks_one() {
        // `None`, not the old `255` sentinel: the relay has not assigned an id yet, and every
        // default draws a palette colour now, so there is nothing to store that means "grey".
        assert_eq!(stored_disc(), None);
        // Whatever id the relay assigns resolves to a real palette colour.
        for seed in ["new-agent-0", "new-group-3", "new-server-1"] {
            assert!(avatars::disc::color_for(seed, stored_disc()) < avatars::disc::COLOR_COUNT);
        }
    }

    #[test]
    fn create_kinds_keep_grok_titles_and_actions() {
        assert_eq!(Kind::Agent.title(), "Create New Agent");
        assert_eq!(Kind::Group.title(), "Create New Group");
        assert_eq!(Kind::Server.title(), "Create New Server");
        assert_eq!(Kind::Connect.title(), "Connect an Agent");
        assert_eq!(Kind::Agent.action(), "Create");
        assert_eq!(Kind::Group.action(), "Next");
        assert_eq!(Kind::Server.action(), "Create");
    }

    #[test]
    fn appearance_body_heights_keep_upload_shorter_than_generate() {
        assert!(UPLOAD_BODY_H < GENERATE_BODY_H);
        assert!(OPTIONS_H + 16.0 + GENERATE_BODY_H < HEIGHT);
    }
}

#[cfg(test)]
mod render_tests {
    //! Headless render of the Create New Agent and Create New Group sheets (the shell's own
    //! `create_entity::sheet` on an idle app state): no shape grid, no swatch, the two options,
    //! Edit-page sections, and the neutral initial in the preview when nothing was picked.

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
    fn both_create_sheets_show_edit_hierarchy_and_two_options(cx: &mut TestAppContext) {
        let (_rt, state, shell, vcx) = mount(cx);
        for kind in [Kind::Agent, Kind::Group] {
            open(kind, "Field notes", &state, &shell, vcx);
            assert!(
                drawn(vcx, "create-entity-card"),
                "{kind:?}: the sheet is up"
            );
            assert!(
                drawn(vcx, "create-entity-preview"),
                "{kind:?}: the hero avatar is drawn"
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
                !drawn(vcx, "agent-disc"),
                "{kind:?}: no coloured disc anywhere on the sheet (no grid, no disc preview)"
            );
            assert!(
                drawn(vcx, "avatar-upload-choose"),
                "{kind:?}: Upload is the first option"
            );
            assert!(
                drawn(vcx, "create-entity-go"),
                "{kind:?}: Create/Next Done pill"
            );
            // Nothing picked → the create carries no picture and the neutral pair.
            let picked = shell.read_with(vcx, |this, _| this.create_picker.draft_picture());
            assert_eq!(picked, None, "{kind:?}");
            shell.update_in(vcx, |this, window, cx| this.cancel_create(kind, window, cx));
            settle(&shell, vcx);
            assert!(!drawn(vcx, "create-entity-card"), "{kind:?}: closed");
        }
    }

    /// Connect an agent (#2141): the same Create screen with the three transport rows under
    /// Connection, the ACP one selected first; Cancel closes it.
    #[gpui::test]
    fn the_connect_sheet_shows_the_transport_rows(cx: &mut TestAppContext) {
        let (_rt, state, shell, vcx) = mount(cx);
        open(Kind::Connect, "Grok Bot", &state, &shell, vcx);
        assert!(drawn(vcx, "create-entity-card"), "the sheet is up");
        assert!(
            drawn(vcx, "connect-transports"),
            "the transport section is drawn"
        );
        for id in [
            "connect-transport-acp",
            "connect-transport-grok",
            "connect-transport-custom",
        ] {
            assert!(drawn(vcx, id), "{id} row");
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

    /// With neither option used, the preview is the neutral initial — no coloured disc at all
    /// on the sheet — and switching to Generate shows the prompt body, still no disc.
    #[gpui::test]
    fn the_preview_falls_back_to_the_neutral_initial(cx: &mut TestAppContext) {
        let (_rt, state, shell, vcx) = mount(cx);
        open(Kind::Agent, "Field notes", &state, &shell, vcx);
        assert!(drawn(vcx, "create-entity-preview"));
        assert!(
            vcx.debug_bounds("agent-disc").is_none(),
            "the neutral preview draws no coloured disc"
        );
        let has_pending = shell.read_with(vcx, |this, cx| {
            avatar_picker::pending_preview(&this.create_picker, this.state.read(cx), PREVIEW)
                .is_some()
        });
        assert!(!has_pending);
        assert_eq!(avatars::disc::initial("Field notes"), "F");
        shell.update(vcx, |this, cx| {
            this.create_picker.tab = Tab::Generate;
            cx.notify();
        });
        settle(&shell, vcx);
        assert!(drawn(vcx, "avatar-generate-go"), "the Generate body is up");
        assert!(drawn(vcx, "avatar-generate-status"));
        assert!(!drawn(vcx, "avatar-upload-choose"));
        assert!(vcx.debug_bounds("agent-disc").is_none());
    }
}

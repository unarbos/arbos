//! S18 draft-and-approve card (feature `approve`), drawn as the Grok Bot
//! question card: the title is the question, the preview (a draft's To /
//! Subject / body or the gated command) is plain text under it, and the
//! answers are two lettered rows — A Approve, B Reject — in the white inner
//! list. The × top-right is Reject. The part shape is owned by
//! `arbos_core::approval`; Approve/Reject write the `approve` / `reject`
//! commands through `AppState`.

use gpui::{div, prelude::*, px, App, Div, SharedString, Stateful};

use crate::model::{ApprovalRisk, ApprovalState};
use crate::ui::theme::{Theme, TEXT_SMALL};
use crate::ui::widgets::{
    card_frame, card_header, option_list, option_row, CardLook, CloseHandler, RowState, ROW_PAD_X,
};

/// Everything the card renders, flattened from `Part::Approval`.
#[derive(Clone, Debug, PartialEq)]
pub struct ApprovalView {
    pub approval_id: String,
    pub title: String,
    pub summary: String,
    pub risk: ApprovalRisk,
    pub state: ApprovalState,
    pub payload: serde_json::Value,
    pub decided_by: Option<String>,
    pub reason: Option<String>,
    /// #1544: the standing choice made on a destructive-shell card (a path class), once decided.
    pub grant: Option<String>,
    /// The agent the card belongs to, for "Always allow Doug in /tmp".
    pub agent: Option<String>,
}

/// One standing option a destructive-shell card offers (#1544): `grant:<path_class>` is the
/// answer id, the label and hint come from the agent's `grant_options`.
#[derive(Clone, Debug, PartialEq)]
pub struct GrantOption {
    pub path_class: String,
    pub label: String,
    pub detail: String,
}

/// Answer ids of a destructive-shell card.
pub const ANSWER_ONCE: &str = "once";
pub const ANSWER_DENY: &str = "deny";
pub const ANSWER_GRANT_PREFIX: &str = "grant:";

impl ApprovalView {
    fn field(&self, key: &str) -> Option<&str> {
        self.payload
            .get(key)
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
    }

    /// Preview lines: a draft shows To / Subject / body; a gated tool call shows the command.
    fn preview(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        match self.field("kind") {
            Some("draft_message") => {
                if let Some(to) = self.field("to") {
                    out.push(("To".into(), to.into()));
                }
                if let Some(subject) = self.field("subject") {
                    out.push(("Subject".into(), subject.into()));
                }
                if let Some(body) = self.field("body") {
                    out.push((String::new(), body.into()));
                }
            }
            Some("tool_call") => {
                if let Some(tool) = self.field("tool") {
                    out.push(("Tool".into(), tool.into()));
                }
                let input = self.payload.get("input");
                let command = input
                    .and_then(|i| i.get("command"))
                    .and_then(|c| c.as_str())
                    .map(str::to_owned)
                    .or_else(|| input.map(|i| i.to_string()));
                if let Some(command) = command {
                    out.push((String::new(), command));
                }
                if let Some(why) = self.field("why") {
                    out.push(("Why".into(), why.into()));
                }
            }
            Some(_) | None => {
                if !self.summary.is_empty() {
                    out.push((String::new(), self.summary.clone()));
                }
            }
        }
        out
    }

    /// The title as a question: "Send the weekly update to the beta list?".
    fn question(&self) -> String {
        let t = self.title.trim();
        if t.is_empty() {
            return "Approve this?".into();
        }
        if t.ends_with('?') {
            t.to_string()
        } else {
            format!("{}?", t.trim_end_matches('.'))
        }
    }
}

impl ApprovalView {
    /// The answers a secret-request card offers (D76 A–D), from `payload.answers`.
    fn answers(&self) -> Vec<(String, String)> {
        self.payload
            .get("answers")
            .and_then(|a| a.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|o| {
                        Some((
                            o.get("id")?.as_str()?.to_string(),
                            o.get("label")?.as_str()?.to_string(),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The standing options of a destructive-shell card, from `payload.grant_options`
    /// (`[{ path_class, label, detail }]`); empty for every other card.
    pub fn grant_options(&self) -> Vec<GrantOption> {
        self.payload
            .get("grant_options")
            .and_then(|a| a.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|o| {
                        Some(GrantOption {
                            path_class: o.get("path_class")?.as_str()?.to_string(),
                            label: o.get("label")?.as_str()?.to_string(),
                            detail: o
                                .get("detail")
                                .and_then(|d| d.as_str())
                                .unwrap_or("")
                                .to_string(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn agent_name(&self) -> String {
        self.agent
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("the agent")
            .to_string()
    }

    /// The four rows of a destructive-shell card (#1544), as `(answer id, label, hint)`:
    /// A Allow once · B Always allow <agent> in <class> · C Always allow <agent> anywhere ·
    /// D Deny. When the command already falls in "anywhere" there is no B.
    pub fn shell_answers(&self) -> Vec<(String, String, String)> {
        let opts = self.grant_options();
        if opts.is_empty() {
            return Vec::new();
        }
        let agent = self.agent_name();
        let mut out = vec![(
            ANSWER_ONCE.to_string(),
            "Allow once".to_string(),
            String::new(),
        )];
        for o in opts {
            let label = if o.path_class == "anywhere" {
                format!("Always allow {agent} anywhere")
            } else {
                format!("Always allow {agent} in {}", o.label)
            };
            let hint = if o.path_class == "anywhere" {
                let class = match self.field("class") {
                    Some("file-delete") => "file deletes",
                    Some(other) => other,
                    None => "this",
                };
                format!("no more cards for {class} from {agent}, in {}", o.detail)
            } else {
                o.detail.clone()
            };
            out.push((
                format!("{ANSWER_GRANT_PREFIX}{}", o.path_class),
                label,
                hint,
            ));
        }
        out.push((ANSWER_DENY.to_string(), "Deny".to_string(), String::new()));
        out
    }

    /// Which row an answered destructive-shell card picked.
    fn picked_shell_answer(&self) -> Option<String> {
        match self.state {
            ApprovalState::Rejected => Some(ANSWER_DENY.to_string()),
            ApprovalState::Approved => Some(match &self.grant {
                Some(class) => format!("{ANSWER_GRANT_PREFIX}{class}"),
                None => ANSWER_ONCE.to_string(),
            }),
            ApprovalState::Pending | ApprovalState::Expired => None,
        }
    }

    /// Which answer an answered secret card picked, from `decidedBy` (`…:allowed_always`).
    fn picked_answer(&self) -> Option<&'static str> {
        match self.state {
            ApprovalState::Rejected => Some("deny"),
            ApprovalState::Approved => {
                let by = self.decided_by.as_deref().unwrap_or("");
                Some(if by.ends_with(":allowed_once") {
                    "once"
                } else if by.ends_with(":allowed_chat") {
                    "chat"
                } else {
                    "always"
                })
            }
            ApprovalState::Pending | ApprovalState::Expired => None,
        }
    }
}

/// The card: question title, plain preview, A Approve / B Reject rows; × = Reject. A secret
/// request (payload `answers`) shows its four rows instead and reports the choice through
/// `on_answer`.
pub fn approval_card(
    row_id: &str,
    view: &ApprovalView,
    theme: &Theme,
    on_approve: impl Fn(&mut App) + 'static,
    on_reject: impl Fn(&mut App) + 'static,
    on_answer: impl Fn(&str, &mut App) + 'static,
) -> Stateful<Div> {
    let look = CardLook::of(theme);
    let palette = look.palette;
    let pending = view.state == ApprovalState::Pending;
    let on_reject = std::rc::Rc::new(on_reject);
    let on_answer = std::rc::Rc::new(on_answer);
    let answers = view.answers();

    let close: Option<CloseHandler> = pending.then(|| {
        let on_reject = on_reject.clone();
        Box::new(move |_: &gpui::ClickEvent, _: &mut gpui::Window, cx: &mut App| on_reject(cx))
            as CloseHandler
    });

    let mut preview = div()
        .flex()
        .flex_col()
        .gap(px(3.0))
        .px(px(1.0))
        .text_size(px(TEXT_SMALL))
        .line_height(px(TEXT_SMALL + 5.0))
        .text_color(theme.text_muted);
    let mut has_preview = false;
    for (label, value) in view.preview() {
        has_preview = true;
        preview = preview.child(if label.is_empty() {
            value
        } else {
            format!("{label}: {value}")
        });
    }

    let shell = view.shell_answers();
    let mut list = option_list(&palette);
    if !shell.is_empty() {
        // #1544: A Allow once · B Always allow <agent> in <class> · C … anywhere · D Deny.
        let picked = view.picked_shell_answer();
        for (ix, (id, label, hint)) in shell.iter().enumerate() {
            let state = if pending {
                RowState::Open
            } else if picked.as_deref() == Some(id.as_str()) {
                RowState::Chosen
            } else {
                RowState::Faded
            };
            let letter = char::from(b'A' + (ix as u8).min(25)).to_string();
            let choice = id.clone();
            let on_answer = on_answer.clone();
            let on_reject = on_reject.clone();
            let deny = id == ANSWER_DENY;
            let row = option_row(
                format!("{row_id}-{}", id.replace(':', "-")),
                Some(letter),
                label,
                state,
                ix > 0,
                &look,
            )
            .debug_selector({
                let id = id.clone();
                move || format!("shell-option-{id}")
            })
            .when(!hint.is_empty(), |el| {
                el.child(
                    div()
                        .flex_shrink_0()
                        .max_w(px(220.0))
                        .text_size(px(TEXT_SMALL - 1.0))
                        .line_height(px(TEXT_SMALL + 3.0))
                        .text_color(theme.text_faint)
                        .text_right()
                        .child(hint.clone()),
                )
            })
            .when(pending, |el| {
                el.on_click(move |_, _, cx| {
                    if deny {
                        on_reject(cx)
                    } else {
                        on_answer(&choice, cx)
                    }
                })
            });
            list = list.child(row);
        }
    } else if answers.is_empty() {
        let (approve_state, reject_state) = match view.state {
            ApprovalState::Pending => (RowState::Open, RowState::Open),
            ApprovalState::Approved => (RowState::Chosen, RowState::Faded),
            ApprovalState::Rejected => (RowState::Faded, RowState::Chosen),
            ApprovalState::Expired => (RowState::Faded, RowState::Faded),
        };
        let approve_row = option_row(
            format!("{row_id}-approve"),
            Some("A".into()),
            "Approve",
            approve_state,
            false,
            &look,
        )
        .when(pending, |el| el.on_click(move |_, _, cx| on_approve(cx)));
        let reject_row = option_row(
            format!("{row_id}-reject"),
            Some("B".into()),
            "Reject",
            reject_state,
            true,
            &look,
        )
        .when(pending, |el| {
            let on_reject = on_reject.clone();
            el.on_click(move |_, _, cx| on_reject(cx))
        });
        list = list.child(approve_row).child(reject_row);
    } else {
        let picked = view.picked_answer();
        for (ix, (id, label)) in answers.iter().enumerate() {
            let state = if pending {
                RowState::Open
            } else if picked == Some(id.as_str()) {
                RowState::Chosen
            } else {
                RowState::Faded
            };
            let letter = char::from(b'A' + (ix as u8).min(25)).to_string();
            let choice = id.clone();
            let on_answer = on_answer.clone();
            list = list.child(
                option_row(
                    format!("{row_id}-{id}"),
                    Some(letter),
                    label,
                    state,
                    ix > 0,
                    &look,
                )
                .when(pending, |el| {
                    el.on_click(move |_, _, cx| on_answer(&choice, cx))
                }),
            );
        }
    }

    if !pending {
        let mut note = view.state.label().to_string();
        if let Some(by) = &view.decided_by {
            note.push_str(&format!(" by {by}"));
        }
        if let Some(reason) = view.reason.as_deref().filter(|r| !r.trim().is_empty()) {
            note.push_str(&format!(" · {reason}"));
        }
        list = list.child(
            div()
                .border_t_1()
                .border_color(palette.hairline)
                .px(px(ROW_PAD_X))
                .py(px(7.0))
                .text_size(px(TEXT_SMALL))
                .text_color(theme.text_faint)
                .child(note),
        );
    }

    let shell_foot = (!shell.is_empty() && pending).then(|| {
        let agent = view.agent_name();
        div()
            .px(px(1.0))
            .text_size(px(TEXT_SMALL - 1.0))
            .line_height(px(TEXT_SMALL + 3.0))
            .text_color(theme.text_faint)
            .debug_selector(|| "shell-card-foot".into())
            .child(format!(
                "Standing choices are per agent: {agent} stops asking for what you allow here."
            ))
    });

    card_frame(SharedString::from(row_id.to_string()), &palette)
        .child(card_header(
            format!("{row_id}-x"),
            &view.question(),
            close,
            &palette,
            theme,
        ))
        .when(has_preview, |el| el.child(preview))
        .child(list)
        .when_some(shell_foot, |el, foot| el.child(foot))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn shell_view(
        state: ApprovalState,
        grant: Option<&str>,
        options: serde_json::Value,
    ) -> ApprovalView {
        ApprovalView {
            approval_id: "ap1".into(),
            title: "Run a destructive command (file-delete)".into(),
            summary: "cd /tmp && rm -rf arbos-src".into(),
            risk: ApprovalRisk::Destructive,
            state,
            payload: json!({ "kind": "tool_call", "tool": "bash", "class": "file-delete",
                "input": { "command": "cd /tmp && rm -rf arbos-src" }, "path_class": "workspace",
                "grant_options": options }),
            decided_by: Some("dev-mac".into()),
            reason: None,
            grant: grant.map(str::to_owned),
            agent: Some("Doug".into()),
        }
    }

    /// #1544: the destructive-shell card offers A once / B in <class> / C anywhere / D deny,
    /// names the agent, and an answered card knows which row was picked.
    #[test]
    fn shell_card_offers_the_four_standing_rows() {
        let opts = json!([
            { "path_class": "workspace", "label": "its workspace", "detail": "the agent's own /work folder, including scratch" },
            { "path_class": "anywhere", "label": "anywhere", "detail": "any folder of its box" }
        ]);
        let v = shell_view(ApprovalState::Pending, None, opts.clone());
        let rows = v.shell_answers();
        let ids: Vec<&str> = rows.iter().map(|(id, _, _)| id.as_str()).collect();
        assert_eq!(ids, ["once", "grant:workspace", "grant:anywhere", "deny"]);
        assert_eq!(rows[1].1, "Always allow Doug in its workspace");
        assert_eq!(rows[2].1, "Always allow Doug anywhere");
        assert!(rows[2]
            .2
            .contains("no more cards for file deletes from Doug"));
        assert_eq!(v.picked_shell_answer(), None);

        let picked = shell_view(ApprovalState::Approved, Some("workspace"), opts.clone());
        assert_eq!(
            picked.picked_shell_answer().as_deref(),
            Some("grant:workspace")
        );
        let once = shell_view(ApprovalState::Approved, None, opts.clone());
        assert_eq!(once.picked_shell_answer().as_deref(), Some("once"));
        let denied = shell_view(ApprovalState::Rejected, None, opts);
        assert_eq!(denied.picked_shell_answer().as_deref(), Some("deny"));

        // A command already in "anywhere" has no B row; a non-path card has no standing rows.
        let wide = shell_view(
            ApprovalState::Pending,
            None,
            json!([{ "path_class": "anywhere", "label": "anywhere", "detail": "any folder of its box" }]),
        );
        assert_eq!(wide.shell_answers().len(), 3);
        let plain = shell_view(ApprovalState::Pending, None, json!([]));
        assert!(plain.shell_answers().is_empty());
    }
}

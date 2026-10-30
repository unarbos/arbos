//! Settings → Machine: live machine details (Jacob 2026-10-26, [feedback
//! machine-page-mac-2026-10-26]; mock approved 14:18 CR as drawn).
//!
//! The page is one grouped list in the pane kit: the **Machine** card (where it runs, state with
//! the uptime folded in, spec, machine time today), **Resources** (disk, memory and CPU as gauge
//! rows with a 3 pt bar, then network), **Agents** (running now, background jobs, a "Per-agent
//! usage" row that unfolds one row per workspace) and the action card (request a bigger machine,
//! restart). Near a limit — disk or memory at [`WARN_PCT`] or more — a warning card leads the page
//! with the two ways out. Numbers come from `GET /machine` (`Machine`) and the box's own
//! `GET /box/state` through `GET /machine/debug` (`BoxReport`), polled every [`POLL_MS`] while the
//! page is open and not a moment longer. Rows whose metric the image does not report yet
//! (an agentd before 0.6.277) are simply not drawn; nothing says "n/a".
//!
//! Pure functions here (rows, gauges, levels, copy, the poll step) are what the tests pin;
//! `me.rs` draws them.

use gpui::{div, prelude::*, px, AnyElement, App, Div};

use crate::model::{duration_s, size_mb, BoxReport, BoxWorkspace, Machine, MachineState};
use crate::ui::profiles::kit;
use crate::ui::sf_symbols;
use crate::ui::theme::{Theme, TEXT_BODY, TEXT_SMALL, TEXT_TINY};

/// Used share at which a gauge turns amber and the warning card appears — agentd's own disk
/// gate (`disk::WARN_PCT`) and the OOM "near cap" rule (#1777) both sit at 85 %.
pub const WARN_PCT: f64 = 85.0;
/// Used share at which a gauge turns red.
pub const CRIT_PCT: f64 = 90.0;
/// `GET /machine/debug` cadence while the page is open.
pub const POLL_MS: u64 = 5_000;
/// The gauge bar under a resource row.
pub const GAUGE_H: f32 = 3.0;
/// File slots are shown only on a filesystem that counts them in a quota-sized way (a 2 GiB
/// work image has 131 072); the Daytona overlay reports the host's 750 M and says nothing.
pub const INODES_SHOWN_BELOW: u64 = 10_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Ok,
    Warn,
    Crit,
}

pub fn level(pct: f64) -> Level {
    if pct >= CRIT_PCT {
        Level::Crit
    } else if pct >= WARN_PCT {
        Level::Warn
    } else {
        Level::Ok
    }
}

/// Used share, 0–100, when a total is known and non-zero.
pub fn pct_used(used: u64, total: u64) -> Option<f64> {
    (total > 0).then(|| (used as f64 / total as f64 * 100.0).clamp(0.0, 100.0))
}

/// One gauge row's numbers.
#[derive(Clone, Debug, PartialEq)]
pub struct Gauge {
    pub pct: f64,
    pub level: Level,
    /// "3.1 GB used of 10 GB" / "23 %".
    pub value: String,
    /// The muted line under the bar, when there is one.
    pub sub: Option<String>,
}

fn used_of(used: u64, total: u64) -> String {
    format!("{} used of {}", size_mb(used), size_mb(total))
}

/// "93 k" / "1.2 k" / "640" — file slots are counted in thousands.
pub fn count_k(n: u64) -> String {
    if n >= 10_000 {
        format!("{} k", n / 1000)
    } else if n >= 1000 {
        format!("{:.1} k", n as f64 / 1000.0)
    } else {
        n.to_string()
    }
}

pub fn disk_gauge(r: &BoxReport) -> Option<Gauge> {
    let (total, avail) = (r.disk_total_mb?, r.disk_available_mb?);
    let used = total.saturating_sub(avail);
    let pct = pct_used(used, total)?;
    let mut sub = format!("{} free", size_mb(avail));
    if let (Some(t), Some(a)) = (r.inodes_total, r.inodes_avail) {
        if t > 0 && t < INODES_SHOWN_BELOW {
            sub.push_str(&format!(" · {} file slots free", count_k(a)));
        }
    }
    Some(Gauge {
        pct,
        level: level(pct),
        value: used_of(used, total),
        sub: Some(sub),
    })
}

pub fn mem_gauge(r: &BoxReport) -> Option<Gauge> {
    let (total, avail) = (r.mem_total_mb?, r.mem_available_mb?);
    let used = total.saturating_sub(avail);
    let pct = pct_used(used, total)?;
    let lvl = level(pct);
    Some(Gauge {
        pct,
        level: lvl,
        value: used_of(used, total),
        sub: (lvl != Level::Ok).then(|| format!("Over {} the machine restarts", size_mb(total))),
    })
}

pub fn cpu_gauge(r: &BoxReport) -> Option<Gauge> {
    let pct = r.cpu_pct?.clamp(0.0, 100.0);
    Some(Gauge {
        pct,
        level: level(pct),
        value: format!("{} %", pct.round() as u64),
        sub: r.cpu_cores.map(|c| {
            if (c - c.round()).abs() < 0.01 {
                format!("{} cores", c.round() as u64)
            } else {
                format!("{c:.1} cores")
            }
        }),
    })
}

/// "1.2 MB/s" / "80 kB/s" / "0 B/s".
pub fn rate(bps: f64) -> String {
    let b = bps.max(0.0);
    if b >= 1e9 {
        format!("{:.1} GB/s", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.1} MB/s", b / 1e6)
    } else if b >= 1e3 {
        format!("{:.0} kB/s", b / 1e3)
    } else {
        format!("{} B/s", b.round() as u64)
    }
}

/// "2.1 GB" / "640 MB" / "12 kB" from bytes.
pub fn bytes(n: u64) -> String {
    let b = n as f64;
    if b >= 1e9 {
        format!("{:.1} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.0} MB", b / 1e6)
    } else if b >= 1e3 {
        format!("{:.0} kB", b / 1e3)
    } else {
        format!("{n} B")
    }
}

/// The Network row: ("↓ 1.2 MB/s  ↑ 80 kB/s", Some("2.1 GB today")).
pub fn net_value(r: &BoxReport) -> Option<(String, Option<String>)> {
    let (rx, tx) = (r.net_rx_bps?, r.net_tx_bps?);
    let today = match (r.net_rx_today, r.net_tx_today) {
        (Some(a), Some(b)) => Some(format!("{} today", bytes(a + b))),
        _ => None,
    };
    Some((format!("↓ {}  ↑ {}", rate(rx), rate(tx)), today))
}

/// The warning card's words when disk or memory is at [`WARN_PCT`] or more.
#[derive(Clone, Debug, PartialEq)]
pub struct Warning {
    pub title: String,
    pub body: String,
    /// The disk and memory shares that tripped it, for the tests and the rig.
    pub disk_pct: Option<f64>,
    pub mem_pct: Option<f64>,
}

pub fn warning(r: &BoxReport) -> Option<Warning> {
    let disk = disk_gauge(r).filter(|g| g.level != Level::Ok);
    let mem = mem_gauge(r).filter(|g| g.level != Level::Ok);
    if disk.is_none() && mem.is_none() {
        return None;
    }
    let title = match (&disk, &mem) {
        (Some(_), Some(_)) => "Running low on disk and memory",
        (Some(_), None) => "Running low on disk",
        (None, Some(_)) => "Running low on memory",
        (None, None) => unreachable!(),
    };
    let mut parts = Vec::new();
    if let Some(g) = &disk {
        parts.push(format!("Disk {} %", g.pct.round() as u64));
    }
    if let Some(g) = &mem {
        parts.push(format!("memory {} %", g.pct.round() as u64));
    }
    let consequence = match (&disk, &mem) {
        (Some(_), Some(_)) => "saves fail and the machine restarts — agents lose their turn",
        (Some(_), None) => "saves fail and agents cannot finish their work",
        (None, Some(_)) => "the machine restarts and agents lose their turn",
        (None, None) => unreachable!(),
    };
    let body = format!("{}. Past the limit, {}.", parts.join(" · "), consequence);
    Some(Warning {
        title: title.into(),
        body,
        disk_pct: disk.map(|g| g.pct),
        mem_pct: mem.map(|g| g.pct),
    })
}

/// The workspace to ask for space: the one using the most disk, when any reports it.
pub fn biggest_disk_user(r: &BoxReport) -> Option<&BoxWorkspace> {
    r.workspaces
        .iter()
        .filter(|w| w.disk_mb.is_some())
        .max_by_key(|w| w.disk_mb.unwrap_or(0))
}

/// The line the composer is prefilled with when the user asks that agent to free space; agentd's
/// disk gate (#1641) answers "free space" / "clean up" itself.
pub const FREE_SPACE_ASK: &str =
    "Please free up disk space in your workspace: remove caches, build outputs and downloads you no longer need, then tell me how much is free.";

/// "3 of 6 agents".
pub fn running_now(r: &BoxReport) -> String {
    let n = r.running().count();
    let total = r.workspaces.len();
    format!("{n} of {total} agent{}", if total == 1 { "" } else { "s" })
}

/// "2 running" / "None".
pub fn jobs_value(jobs: u64) -> String {
    match jobs {
        0 => "None".into(),
        n => format!("{n} running"),
    }
}

/// "Working · 42 s · 1 job" / "Idle" (the tool count stays on the Computer tab; the
/// breakdown row's right column needs the room).
pub fn agent_status(w: &BoxWorkspace) -> String {
    if !w.running {
        return match w.jobs_running {
            Some(j) if j > 0 => format!("Idle · {j} job{}", if j == 1 { "" } else { "s" }),
            _ => "Idle".into(),
        };
    }
    let mut parts = vec!["Working".to_string()];
    if let Some(ms) = w.elapsed_ms {
        parts.push(duration_s(ms / 1000));
    }
    if let Some(j) = w.jobs_running.filter(|j| *j > 0) {
        parts.push(format!("{j} job{}", if j == 1 { "" } else { "s" }));
    }
    parts.join(" · ")
}

/// "CPU 61 %".
pub fn agent_cpu(w: &BoxWorkspace) -> Option<String> {
    w.cpu_pct.map(|p| format!("CPU {} %", p.round() as u64))
}

/// "1.9 GB · 2.2 GB disk".
pub fn agent_mem_disk(w: &BoxWorkspace) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(m) = w.mem_mb {
        parts.push(size_mb(m));
    }
    if let Some(d) = w.disk_mb {
        parts.push(format!("{} disk", size_mb(d)));
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// Everything the page may draw, in order, by card. Pure, so the order and the fallbacks can be
/// pinned without a window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    // warning card
    Warning,
    AskToFreeSpace,
    RequestBiggerMachineInWarning,
    // machine card
    WhereItRuns,
    InLine,
    State,
    Spec,
    MachineTime,
    // resources card
    Disk,
    Memory,
    Cpu,
    Network,
    NotAnswering,
    // agents card
    RunningNow,
    Jobs,
    Breakdown,
    Agent,
    // action card
    RequestBiggerMachine,
    Restart,
    Start,
    Stop,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub warning: Vec<Row>,
    pub machine: Vec<Row>,
    pub resources: Vec<Row>,
    pub agents: Vec<Row>,
    pub actions: Vec<Row>,
}

pub fn plan(machine: &Machine, report: Option<&BoxReport>, breakdown_open: bool) -> Plan {
    let mut p = Plan::default();
    let live = report.filter(|r| r.reachable);
    if let Some(r) = live {
        if warning(r).is_some() {
            p.warning.push(Row::Warning);
            if biggest_disk_user(r).is_some() && warning(r).and_then(|w| w.disk_pct).is_some() {
                p.warning.push(Row::AskToFreeSpace);
            }
            p.warning.push(Row::RequestBiggerMachineInWarning);
        }
    }
    p.machine.push(Row::WhereItRuns);
    if machine.waitlist_position.is_some() {
        p.machine.push(Row::InLine);
    }
    p.machine.extend([Row::State, Row::Spec, Row::MachineTime]);
    match live {
        Some(r) => {
            if disk_gauge(r).is_some() {
                p.resources.push(Row::Disk);
            }
            if mem_gauge(r).is_some() {
                p.resources.push(Row::Memory);
            }
            if cpu_gauge(r).is_some() {
                p.resources.push(Row::Cpu);
            }
            if net_value(r).is_some() {
                p.resources.push(Row::Network);
            }
            p.agents.push(Row::RunningNow);
            if r.jobs_running().is_some() {
                p.agents.push(Row::Jobs);
            }
            if !r.usage_rows().is_empty() {
                p.agents.push(Row::Breakdown);
                if breakdown_open {
                    p.agents
                        .extend(std::iter::repeat_n(Row::Agent, r.usage_rows().len()));
                }
            }
        }
        None => {
            if report.is_some() && machine.shown_state() == MachineState::Awake {
                p.resources.push(Row::NotAnswering);
            }
        }
    }
    if machine.always_on {
        p.actions.push(Row::RequestBiggerMachine);
        p.actions.push(Row::Restart);
    } else if machine.state.is_down() {
        p.actions.push(Row::Start);
    } else {
        p.actions.push(Row::Stop);
    }
    p
}

/// The 5-second poll, decided from what the pane shows: the page starts it, leaving the page
/// stops it, and a running poll ticks while the page stays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PollStep {
    Start,
    Tick,
    Stop,
    Idle,
}

pub fn poll_step(polling: bool, on_page: bool) -> PollStep {
    match (polling, on_page) {
        (false, true) => PollStep::Start,
        (true, true) => PollStep::Tick,
        (true, false) => PollStep::Stop,
        (false, false) => PollStep::Idle,
    }
}

// ---- drawing ------------------------------------------------------------------------------------

fn level_color(level: Level, theme: &Theme) -> gpui::Hsla {
    match level {
        Level::Ok => theme.label_faint(),
        Level::Warn => theme.warn,
        Level::Crit => theme.danger,
    }
}

/// A gauge row: title left, value right (coloured past the thresholds), a 3 pt bar under both,
/// an optional muted line under the bar. The kit's divider hangs under it like every row.
pub fn gauge_row(title: &str, g: &Gauge, theme: &Theme) -> Div {
    let color = level_color(g.level, theme);
    let value_color = match g.level {
        Level::Ok => theme.label_muted(),
        _ => color,
    };
    div()
        .relative()
        .flex()
        .flex_col()
        .gap(px(7.0))
        .min_h(px(kit::ROW_MIN_H))
        .px(px(kit::INSET))
        .py(px(9.0))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(12.0))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(TEXT_BODY))
                        .text_color(theme.text)
                        .child(title.to_string()),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(TEXT_BODY))
                        .text_color(value_color)
                        .child(g.value.clone()),
                ),
        )
        .child(
            div()
                .h(px(GAUGE_H))
                .w_full()
                .rounded(px(GAUGE_H / 2.0))
                .bg(Theme::with_alpha(theme.text, 0.09))
                .overflow_hidden()
                .child(
                    div()
                        .h_full()
                        .w(gpui::relative((g.pct / 100.0) as f32))
                        .rounded(px(GAUGE_H / 2.0))
                        .bg(color),
                ),
        )
        .when_some(g.sub.clone(), |el, sub| {
            el.child(
                div()
                    .mt(px(-2.0))
                    .text_size(px(TEXT_SMALL))
                    .text_color(theme.label_muted())
                    .truncate()
                    .child(sub),
            )
        })
        .child(kit::hairline_below(theme))
}

/// The warning row at the top of the page: amber glyph, bold title, one plain sentence.
pub fn warning_row(w: &Warning, theme: &Theme) -> Div {
    div()
        .relative()
        .flex()
        .items_start()
        .gap(px(12.0))
        .px(px(kit::INSET))
        .py(px(10.0))
        .child(div().flex_shrink_0().mt(px(1.0)).child(sf_symbols::glyph(
            "exclamationmark.circle",
            22.0,
            theme.warn,
        )))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .text_size(px(TEXT_BODY))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(theme.text)
                        .child(w.title.clone()),
                )
                .child(
                    div()
                        .text_size(px(TEXT_SMALL))
                        .text_color(theme.label_muted())
                        .child(w.body.clone()),
                ),
        )
        .child(kit::hairline_below(theme))
}

/// One workspace in the breakdown: picture, name over its status, CPU over memory · disk.
/// A running workspace leads its status with the working spinner (`ui::working`, the
/// sidebar rows' one — Jacob, 2026-10-27).
pub fn agent_row(picture: AnyElement, w: &BoxWorkspace, theme: &Theme, cx: &mut App) -> Div {
    div()
        .relative()
        .flex()
        .items_center()
        .gap(px(10.0))
        .min_h(px(kit::ROW_MIN_H))
        .px(px(kit::INSET))
        .py(px(7.0))
        .child(picture)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .text_size(px(TEXT_BODY))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(theme.text)
                        .truncate()
                        .child(w.name.clone()),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .min_w_0()
                        .when(w.running, |el| {
                            el.child(crate::ui::working::glyph(theme, cx))
                        })
                        .child(
                            div()
                                .text_size(px(TEXT_TINY))
                                .text_color(theme.label_muted())
                                .truncate()
                                .child(agent_status(w)),
                        ),
                ),
        )
        .child(
            div()
                .flex_shrink_0()
                .flex()
                .flex_col()
                .items_end()
                .gap(px(2.0))
                .when_some(agent_cpu(w), |el, c| {
                    el.child(
                        div()
                            .text_size(px(TEXT_SMALL))
                            .text_color(theme.label_muted())
                            .child(c),
                    )
                })
                .when_some(agent_mem_disk(w), |el, m| {
                    el.child(
                        div()
                            .text_size(px(TEXT_TINY))
                            .text_color(theme.label_muted())
                            .child(m),
                    )
                }),
        )
        .child(kit::hairline_below(theme))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(disk_avail: u64, mem_avail: u64) -> BoxReport {
        BoxReport {
            reachable: true,
            disk_total_mb: Some(10 * 1024),
            disk_available_mb: Some(disk_avail),
            mem_total_mb: Some(8 * 1024),
            mem_available_mb: Some(mem_avail),
            cpu_cores: Some(4.0),
            cpu_pct: Some(23.4),
            inodes_total: Some(131_072),
            inodes_avail: Some(93_000),
            net_rx_bps: Some(1_200_000.0),
            net_tx_bps: Some(80_000.0),
            net_rx_today: Some(2_000_000_000),
            net_tx_today: Some(100_000_000),
            workspaces: vec![
                BoxWorkspace {
                    project_id: "p1".into(),
                    name: "Arbos".into(),
                    healthy: true,
                    running: true,
                    elapsed_ms: Some(42_000),
                    tool_calls: Some(7),
                    jobs_running: Some(1),
                    cpu_pct: Some(61.0),
                    mem_mb: Some(1_900),
                    pids: Some(40),
                    disk_mb: Some(2_300),
                },
                BoxWorkspace {
                    project_id: "p2".into(),
                    name: "Comms".into(),
                    healthy: true,
                    running: false,
                    jobs_running: Some(0),
                    cpu_pct: Some(0.0),
                    mem_mb: Some(600),
                    disk_mb: Some(400),
                    ..Default::default()
                },
            ],
            ..Default::default()
        }
    }

    fn hosted() -> Machine {
        Machine {
            always_on: true,
            state: MachineState::Awake,
            provider: Some("daytona".into()),
            ..Default::default()
        }
    }

    #[test]
    fn thresholds_are_85_amber_and_90_red() {
        assert_eq!(level(84.9), Level::Ok);
        assert_eq!(level(85.0), Level::Warn);
        assert_eq!(level(89.9), Level::Warn);
        assert_eq!(level(90.0), Level::Crit);
        assert_eq!(pct_used(9_216, 10_240), Some(90.0));
        assert_eq!(pct_used(1, 0), None);
    }

    #[test]
    fn gauges_read_used_of_total_and_the_disk_names_file_slots() {
        let r = report(6_900, 5_300);
        let d = disk_gauge(&r).unwrap();
        assert_eq!(d.value, "3.3 GB used of 10 GB");
        assert_eq!(d.level, Level::Ok);
        assert_eq!(d.sub.as_deref(), Some("6.7 GB free · 93 k file slots free"));
        let m = mem_gauge(&r).unwrap();
        assert_eq!(m.value, "2.8 GB used of 8.0 GB");
        assert_eq!(m.sub, None);
        let c = cpu_gauge(&r).unwrap();
        assert_eq!(c.value, "23 %");
        assert_eq!(c.sub.as_deref(), Some("4 cores"));
        // Daytona's overlay counts the host's 750 M inodes: no file-slot line.
        let mut host = report(6_900, 5_300);
        host.inodes_total = Some(750_132_736);
        assert_eq!(
            disk_gauge(&host).unwrap().sub.as_deref(),
            Some("6.7 GB free")
        );
    }

    #[test]
    fn network_row_is_rates_and_bytes_today() {
        let (v, today) = net_value(&report(6_900, 5_300)).unwrap();
        assert_eq!(v, "↓ 1.2 MB/s  ↑ 80 kB/s");
        assert_eq!(today.as_deref(), Some("2.1 GB today"));
        assert_eq!(rate(0.0), "0 B/s");
        assert_eq!(bytes(640_000_000), "640 MB");
    }

    #[test]
    fn warning_appears_at_the_threshold_with_the_two_ways_out() {
        assert_eq!(warning(&report(6_900, 5_300)), None);
        // disk 92 % (0.8 GB free of 10), memory 88 % (1.0 GB free of 8)
        let r = report(819, 983);
        let w = warning(&r).expect("near limit");
        assert_eq!(w.title, "Running low on disk and memory");
        assert_eq!(
            w.body,
            "Disk 92 % · memory 88 %. Past the limit, saves fail and the machine restarts — agents lose their turn."
        );
        assert_eq!(disk_gauge(&r).unwrap().level, Level::Crit);
        assert_eq!(mem_gauge(&r).unwrap().level, Level::Warn);
        assert_eq!(
            mem_gauge(&r).unwrap().sub.as_deref(),
            Some("Over 8.0 GB the machine restarts")
        );
        let p = plan(&hosted(), Some(&r), false);
        assert_eq!(
            p.warning,
            [
                Row::Warning,
                Row::AskToFreeSpace,
                Row::RequestBiggerMachineInWarning
            ]
        );
        assert_eq!(biggest_disk_user(&r).unwrap().name, "Arbos");
        // Memory alone: no "ask to free space" (it is a disk remedy).
        let mem_only = report(6_900, 983);
        let p = plan(&hosted(), Some(&mem_only), false);
        assert_eq!(
            p.warning,
            [Row::Warning, Row::RequestBiggerMachineInWarning]
        );
        assert_eq!(warning(&mem_only).unwrap().title, "Running low on memory");
    }

    #[test]
    fn plan_order_live_box() {
        let r = report(6_900, 5_300);
        let p = plan(&hosted(), Some(&r), false);
        assert!(p.warning.is_empty());
        assert_eq!(
            p.machine,
            [Row::WhereItRuns, Row::State, Row::Spec, Row::MachineTime]
        );
        assert_eq!(
            p.resources,
            [Row::Disk, Row::Memory, Row::Cpu, Row::Network]
        );
        assert_eq!(p.agents, [Row::RunningNow, Row::Jobs, Row::Breakdown]);
        assert_eq!(p.actions, [Row::RequestBiggerMachine, Row::Restart]);
        assert_eq!(running_now(&r), "1 of 2 agents");
        assert_eq!(jobs_value(r.jobs_running().unwrap()), "1 running");
    }

    #[test]
    fn breakdown_expands_to_one_row_per_workspace_biggest_first() {
        let r = report(6_900, 5_300);
        let open = plan(&hosted(), Some(&r), true);
        assert_eq!(
            open.agents,
            [
                Row::RunningNow,
                Row::Jobs,
                Row::Breakdown,
                Row::Agent,
                Row::Agent
            ]
        );
        let closed = plan(&hosted(), Some(&r), false);
        assert!(!closed.agents.contains(&Row::Agent));
        let rows = r.usage_rows();
        assert_eq!(rows[0].name, "Arbos");
        assert_eq!(agent_status(rows[0]), "Working · 42 s · 1 job");
        assert_eq!(agent_cpu(rows[0]).as_deref(), Some("CPU 61 %"));
        assert_eq!(
            agent_mem_disk(rows[0]).as_deref(),
            Some("1.9 GB · 2.2 GB disk")
        );
        assert_eq!(agent_status(rows[1]), "Idle");
    }

    #[test]
    fn an_older_image_draws_only_what_it_reports() {
        // agentd < 0.6.277: disk + memory (host's on Daytona, but that is what it says), no CPU,
        // no network, no breakdown, no jobs.
        let r = BoxReport {
            reachable: true,
            disk_total_mb: Some(10_240),
            disk_available_mb: Some(6_900),
            mem_total_mb: Some(8_192),
            mem_available_mb: Some(5_300),
            load_1m: Some(0.42),
            workspaces: vec![BoxWorkspace {
                project_id: "p1".into(),
                name: "Arbos".into(),
                healthy: true,
                running: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        let p = plan(&hosted(), Some(&r), true);
        assert_eq!(p.resources, [Row::Disk, Row::Memory]);
        assert_eq!(p.agents, [Row::RunningNow]);
        assert!(r.usage_rows().is_empty());
        assert_eq!(r.jobs_running(), None);
    }

    #[test]
    fn an_unanswering_box_says_so_once_and_no_report_says_nothing() {
        let asleep = BoxReport {
            reachable: false,
            unreachable_reason: Some("box unreachable: timeout".into()),
            ..Default::default()
        };
        let p = plan(&hosted(), Some(&asleep), false);
        assert_eq!(p.resources, [Row::NotAnswering]);
        assert!(p.agents.is_empty());
        let p = plan(&hosted(), None, false);
        assert!(p.resources.is_empty());
        assert!(p.agents.is_empty());
        assert_eq!(
            p.machine,
            [Row::WhereItRuns, Row::State, Row::Spec, Row::MachineTime]
        );
    }

    #[test]
    fn a_self_hosted_machine_keeps_start_and_stop() {
        let mut m = hosted();
        m.always_on = false;
        m.state = MachineState::Asleep;
        assert_eq!(plan(&m, None, false).actions, [Row::Start]);
        m.state = MachineState::Awake;
        assert_eq!(plan(&m, None, false).actions, [Row::Stop]);
    }

    #[test]
    fn the_poll_starts_with_the_page_and_stops_when_it_leaves() {
        assert_eq!(poll_step(false, true), PollStep::Start);
        assert_eq!(poll_step(true, true), PollStep::Tick);
        assert_eq!(poll_step(true, false), PollStep::Stop);
        assert_eq!(poll_step(false, false), PollStep::Idle);
        assert_eq!(POLL_MS, 5_000);
    }

    #[test]
    fn counts_in_thousands() {
        assert_eq!(count_k(93_000), "93 k");
        assert_eq!(count_k(1_234), "1.2 k");
        assert_eq!(count_k(640), "640");
    }
}

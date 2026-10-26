//! The avatar picker's Upload preview draws the mastered picture, not the raw file (Jacob,
//! 2026-10-26: a 4000 px mark through the GPU sampler came out stair-stepped). The real `Shell`
//! headless over an `AppState`: choosing a file asks the sync client for `file://<path>`, the
//! preview is empty until the master lands, then shows it — at ≤ 1024 px and shaded edges.

use futures::future::BoxFuture;
use gpui::{
    point, AppContext as _, Bounds, Entity, Modifiers, Pixels, TestAppContext, VisualTestContext,
};
use tokio::sync::mpsc;

use crate::app::{AppState, ProfilePage};
use crate::model::AccountView;
use crate::sync::{ClientCommand, SyncEvent, SyncSource};
use crate::ui::avatar_picker::{self, Tab};
use crate::ui::motion::PulseClock;
use crate::ui::shell::Shell;
use crate::ui::theme::Theme;

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
    let me = AccountView {
        id: "gh_1".into(),
        login: "unarbos".into(),
        name: "Arbos".into(),
        avatar_url: String::new(),
        device_name: "Test".into(),
        balance_usd: 1.0,
        spent_today_usd: 0.0,
        daily_cap_usd: None,
    };
    state.update(cx, |s, cx| {
        s.apply(SyncEvent::Account(Some(me)), cx);
        s.open_profile(ProfilePage::Me, cx);
    });
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

fn centre(b: Bounds<Pixels>) -> gpui::Point<Pixels> {
    point(b.left() + b.size.width / 2.0, b.top() + b.size.height / 2.0)
}

fn click(vcx: &mut VisualTestContext, shell: &Entity<Shell>, id: &'static str) {
    let b = vcx
        .debug_bounds(id)
        .unwrap_or_else(|| panic!("{id} is on screen"));
    vcx.simulate_click(centre(b), Modifiers::default());
    settle(shell, vcx);
}

/// A hard-edged black ring on white, `side` px, written as a PNG file.
fn mark_file(side: u32) -> std::path::PathBuf {
    let c = side as f32 / 2.0;
    let mut img = image::RgbaImage::from_pixel(side, side, image::Rgba([255, 255, 255, 255]));
    for (x, y, p) in img.enumerate_pixels_mut() {
        let (dx, dy) = (x as f32 + 0.5 - c, y as f32 + 0.5 - c);
        let d = (dx * dx + dy * dy).sqrt();
        if (d <= side as f32 * 0.46 && d >= side as f32 * 0.40)
            || ((dx - dy).abs() < side as f32 * 0.04 && d < side as f32 * 0.38)
        {
            *p = image::Rgba([0, 0, 0, 255]);
        }
    }
    let path = std::env::temp_dir().join(format!(
        "arbos-upload-preview-{}-{side}.png",
        std::process::id()
    ));
    img.save(&path).unwrap();
    path
}

#[gpui::test]
fn the_upload_preview_draws_the_mastered_picture_not_the_raw_file(cx: &mut TestAppContext) {
    let (_rt, state, shell, vcx) = mount(cx);
    let mut commands = state.update(vcx, |s, _| s.take_command_channel());
    settle(&shell, vcx);

    click(vcx, &shell, "me-avatar");
    click(vcx, &shell, "avatar-tab-upload");
    assert!(shell.read_with(&*vcx, |sh, _| sh
        .avatar_picker
        .as_ref()
        .is_some_and(|pk| pk.tab == Tab::Upload)));

    // The panel's pick: the picker records the file and asks the loader for `file://…`.
    let path = mark_file(2400);
    let url = avatar_picker::file_url(&path);
    shell.update(vcx, |sh, cx| sh.avatar_file_chosen(path.clone(), cx));
    settle(&shell, vcx);
    let asked = std::iter::from_fn(|| commands.try_recv().ok())
        .find(|c| matches!(c, ClientCommand::FetchAvatar { url: u } if *u == url));
    assert!(
        asked.is_some(),
        "the preview is fetched through the avatar cache: {url}"
    );
    assert!(
        vcx.debug_bounds("avatar-upload-preview").is_none(),
        "nothing is drawn from the raw file while the master is being made"
    );
    assert!(shell.read_with(&*vcx, |sh, _| sh
        .avatar_picker
        .as_ref()
        .is_some_and(|pk| pk.dirty())));

    // The sync client answers with the master (what `fetch_avatar` sends for `file://`).
    let master = crate::sync::avatar_master::master(&std::fs::read(&path).unwrap()).unwrap();
    assert!(master.png, "a two-colour mark stays PNG");
    state.update(vcx, |s, cx| {
        s.apply(
            SyncEvent::Avatar {
                url: url.clone(),
                bytes: std::sync::Arc::new(master.bytes.clone()),
            },
            cx,
        )
    });
    settle(&shell, vcx);
    let preview = vcx
        .debug_bounds("avatar-upload-preview")
        .expect("the mastered picture previews");
    assert!(f32::from(preview.size.width) > 100.0);
    let decoded = state
        .read_with(&*vcx, |s, _| s.avatar(&url))
        .expect("cached");
    assert_eq!(
        decoded.size(0).width.0,
        1024,
        "the cache holds the ≤ 1024 px master, not 2400 px"
    );
    // What the disc actually draws: the display-size copy, shaded edges.
    crate::ui::picture_fit::note_scale_factor(2.0);
    let drawn = crate::ui::picture_fit::fit(&decoded, 114.0);
    assert_eq!(drawn.size(0).width.0, 228);

    // Set → the upload command carries the chosen file (the relay client masters it again).
    click(vcx, &shell, "avatar-set");
    let set = std::iter::from_fn(|| commands.try_recv().ok())
        .find(|c| !matches!(c, ClientCommand::FetchAvatar { .. }));
    match set {
        Some(ClientCommand::SetMyAvatar { path: p }) => assert_eq!(p, path),
        other => panic!("expected SetMyAvatar, got {other:?}"),
    }
    let _ = std::fs::remove_file(&path);
}

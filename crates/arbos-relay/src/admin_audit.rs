//! One structured line for every admin action, on one target and at one level.
//!
//! The relay keeps no access log. An admin token could call any of the ~45 admin-reachable
//! routes and leave no trace: five handlers logged something, the rest logged nothing at any
//! level. The fix is a tower layer over the whole router rather than a `tracing::info!` per
//! handler — a route added tomorrow is audited because it is in the router, not because its
//! author remembered.
//!
//! The line is keyed on the *principal*, not on the path. "An admin route" has been miscounted
//! more than once here, because `Principal::Admin` is accepted well outside the `/admin` prefix
//! — `/blob/…`, `/projects/…/local/…/exec`, `/room/…`. Keying on who authenticated instead of
//! where they landed removes the question.
//!
//! A request layer is not enough on its own. `WS /room/{project}/{chat}` authenticates once and
//! then carries every later admin action inside the open socket, where there is no further
//! request to intercept, so [`log_admin_socket_open`] and [`log_admin_row_push`] cover the
//! inside of that socket. They are the only two call sites outside this module.

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;

use arbos_core::ids::now_ms;

use crate::auth::Principal;
use crate::state::AppState;

/// The one target every admin line carries. Set explicitly, not left to the module path, so the
/// target survives this code moving and so `journalctl … | grep arbos_relay::admin_audit` is the
/// whole query. Under `arbos_relay` so the relay's default filter (`arbos_relay=debug`) and a
/// bare `info` filter both keep it.
pub(crate) const TARGET: &str = "arbos_relay::admin_audit";

/// One HTTP request that authenticated as the admin.
pub(crate) const MESSAGE: &str = "admin request";

/// One room socket opened by the admin. The upgrade is itself an HTTP request, so the layer
/// above already writes a `status=101` line for it; this one names the event and splits out the
/// project and chat, so the rows below it can be read as a session rather than inferred from a
/// status code.
pub(crate) const MESSAGE_SOCKET: &str = "admin room socket opened";

/// One row the admin pushed through an open socket. There is no request behind this, so no
/// request layer can see it.
pub(crate) const MESSAGE_ROW: &str = "admin room row";

/// `p_abc/c_xyz` → `("p_abc", "c_xyz")`. Room names are `{project}/{chat}`.
fn split_room(room_name: &str) -> (&str, &str) {
    room_name.rsplit_once('/').unwrap_or((room_name, "-"))
}

/// Axum middleware: one `admin request` line per request that authenticated as the admin.
///
/// Logged: `route` (path only), `method`, `caller`, `status`, `ts`. Never the token, and never
/// the query string — the relay also accepts the bearer as `?token=…` on WebSocket dials
/// (`auth::token_from`), so a query string can be the credential itself.
pub(crate) async fn log_admin_requests(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    let route = req.uri().path().to_owned();
    let method = req.method().clone();
    // Cheap enough to run on every request: a string compare against the configured token, no
    // database. Non-admin tokens are never resolved here, so the audit adds no reads or writes.
    let is_admin = crate::auth::token_from_parts_ref(&req)
        .is_some_and(|t| crate::auth::is_admin_token(&state, &t));

    let res = next.run(req).await;

    if is_admin {
        tracing::info!(
            target: TARGET,
            %route,
            %method,
            // The principal's own label, so the field follows `Principal` if the relay ever
            // grows more than one admin. It is deliberately not a token fingerprint: there is
            // exactly one admin token (`Config::admin_token`), so a fingerprint would
            // distinguish nothing while writing a live credential's hash into the journal, and
            // hashes are lookup keys in this relay (`db_accounts::refresh_row`).
            caller = %Principal::Admin.device_name(),
            status = res.status().as_u16(),
            ts = now_ms(),
            "{MESSAGE}"
        );
    }
    res
}

/// One line when the admin opens a room socket (`WS /room/{project}/{chat}`).
///
/// That socket is gated only by `AppState::allows`, which is `true` for every admin
/// (`state.rs`), and it is authenticated once: everything afterwards travels inside it.
pub(crate) fn log_admin_socket_open(principal: &Principal, room_name: &str) {
    if !matches!(principal, Principal::Admin) {
        return;
    }
    let (project, chat) = split_room(room_name);
    tracing::info!(
        target: TARGET,
        caller = %principal.device_name(),
        %project,
        %chat,
        ts = now_ms(),
        "{MESSAGE_SOCKET}"
    );
}

/// One line per row the admin appends through an open socket.
///
/// `claimed_author` is the author id the row is stored and broadcast under. The relay takes it
/// from the caller's own `Hello` frame and never checks it against the principal (`rooms.rs`), so
/// an admin can write a row that reads as any device — including a real device of the person
/// being written about. `author_verified` is that check, done here for the log only: it is the
/// difference between "an admin pushed a row" and "an admin pushed a row under someone's name".
/// Nothing here refuses the row; this records what happened and changes no behaviour.
pub(crate) fn log_admin_row_push(
    principal: &Principal,
    room_name: &str,
    claimed_author: &str,
    seq: i64,
) {
    if !matches!(principal, Principal::Admin) {
        return;
    }
    let (project, chat) = split_room(room_name);
    tracing::info!(
        target: TARGET,
        caller = %principal.device_name(),
        %project,
        %chat,
        seq,
        claimed_author = %claimed_author,
        author_verified = claimed_author == principal.device_name(),
        ts = now_ms(),
        "{MESSAGE_ROW}"
    );
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::sync::{Arc, Mutex};

    use axum::http::{Method, StatusCode};
    use tracing_subscriber::layer::SubscriberExt;

    use super::*;
    use crate::state::test_support::test_state;
    use crate::state::AuthMode;

    /// The admin token `test_state` configures.
    const ADMIN: &str = "adm_test";

    /// Every admin line sits at this level, and the tests below hold it there. `rooms.rs` has
    /// instrumented joins and leaves all along at `debug!`, and production runs
    /// `arbos_relay=info` (`scripts/deploy/relay-server.sh`), so those lines are discarded before
    /// they are written. Instrumented in code and absent in production passes review; it must not
    /// happen to this audit.
    const LEVEL: tracing::Level = tracing::Level::INFO;

    /// Stands in for every `{param}` segment when a declared path is made concrete. Not a value
    /// any static route uses, so a parameterised route is never shadowed by a literal one.
    const PARAM: &str = "01testparam";

    /// The router source. The coverage test reads the declared paths out of this rather than out
    /// of a list someone maintains by hand, so a `.route(…)` added to the relay joins the test
    /// without anyone touching the test.
    const ROUTER_SRC: &str = include_str!("main.rs");

    /// The deploy script that writes the relay's environment file, read at compile time so the
    /// filter this is tested against is the filter production actually runs.
    const RELAY_SERVER_SH: &str = include_str!("../../../scripts/deploy/relay-server.sh");

    /// Routes the audit must cover that are not named `/admin/…`, each a reviewer's first
    /// question. Pinned by their declared path, so renaming one is a decision and not a drift:
    /// the room socket (authenticated once, then unlimited admin actions inside it), the local
    /// exec route (`Box|Admin`, arbitrary shell, hands back a device token), and the blob read.
    const MUST_COVER: [&str; 3] = [
        "/room/{project}/{chat}",
        "/projects/{id}/local/{device}/exec",
        "/blob/{project}/{key}",
    ];

    /// The journal of one test, in memory.
    #[derive(Clone, Default)]
    struct Journal(Arc<Mutex<Vec<u8>>>);

    impl Journal {
        fn text(&self) -> String {
            String::from_utf8_lossy(&self.0.lock().expect("journal")).into_owned()
        }

        /// Just the admin lines, in order.
        fn admin_lines(&self) -> Vec<String> {
            self.text()
                .lines()
                .filter(|l| l.contains(TARGET))
                .map(str::to_owned)
                .collect()
        }
    }

    impl io::Write for Journal {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().expect("journal").extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Journal {
        type Writer = Journal;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    /// Capture this thread's journal for as long as the guard lives.
    fn capture() -> (Journal, tracing::subscriber::DefaultGuard) {
        let journal = Journal::default();
        let sub = tracing_subscriber::fmt()
            .with_writer(journal.clone())
            .with_ansi(false)
            .with_max_level(tracing::Level::INFO)
            .finish();
        let guard = tracing::subscriber::set_default(sub);
        (journal, guard)
    }

    /// One route as the router declares it.
    struct Declared {
        path: String,
        /// Declared with `any(…)`, so it answers every method and the probe below reaches its
        /// handler instead of the method router's 405.
        any_method: bool,
    }

    /// The router source with its comment lines dropped, so prose that happens to mention
    /// `.route(` — including the comment on `build_app` itself — is not read as a route.
    fn router_code() -> String {
        ROUTER_SRC
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Every path `build_app` declares, read out of the router source.
    fn declared_routes() -> Vec<Declared> {
        let code = router_code();
        let mut out = Vec::new();
        for entry in code.split(".route(").skip(1) {
            let rest = entry.trim_start();
            let lit = rest
                .strip_prefix('"')
                .unwrap_or_else(|| panic!("`.route(` without a path literal near: {:.60}", rest));
            let end = lit.find('"').expect("unterminated path literal");
            out.push(Declared {
                path: lit[..end].to_owned(),
                any_method: entry.contains("any("),
            });
        }
        out
    }

    /// `/admin/users/{id}` → `/admin/users/01testparam`, `/_matrix/client/{*rest}` likewise.
    fn concrete(path: &str) -> String {
        path.split('/')
            .map(|seg| if seg.starts_with('{') { PARAM } else { seg })
            .collect::<Vec<_>>()
            .join("/")
    }

    /// Serve the real router on a loopback port, as `site_icons`, `gifs`, `proxy` and
    /// `preview_token_tests` already do. Deliberately not `tower::ServiceExt::oneshot`: that
    /// needs a dev-dependency, and a dependency is a line in `Cargo.lock`, which is in the
    /// trigger path list of both `ios.yml` and `swift-core-check.yml`. Going through
    /// `axum::serve` costs a port and buys the production serve path.
    async fn serve(state: AppState) -> (String, reqwest::Client) {
        let app = crate::build_app(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move { axum::serve(listener, app).await.ok() });
        let client = reqwest::Client::builder()
            // A redirect would be a second request and so a second line; keep one call to one line.
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .expect("client");
        (format!("http://{addr}"), client)
    }

    async fn call(
        client: &reqwest::Client,
        base: &str,
        method: Method,
        path: &str,
        bearer: Option<&str>,
    ) -> StatusCode {
        let mut req = client.request(method, format!("{base}{path}"));
        if let Some(t) = bearer {
            req = req.header("authorization", format!("Bearer {t}"));
        }
        req.send().await.expect("response").status()
    }

    /// A **current-thread** runtime, which this capture depends on: `set_default` installs the
    /// subscriber on one thread, and only a current-thread runtime keeps the spawned server task
    /// on that same thread. A multi-threaded flavour here would silently capture nothing.
    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
    }

    /// Jacob's requirement: the test has to fail if any admin route skips the line. It takes the
    /// route set from the router instead of from a list, and drives every one of them through the
    /// real `build_app` router, so a route that bypassed the layer would show up here as a miss.
    ///
    /// The probe is `TRACE`, which no route declares: the method router answers 405 without
    /// running the handler, so the whole router can be swept without restarting the relay,
    /// publishing a release or flipping the kill switch. 405 rather than 404 is itself the proof
    /// that the path reached its route and not the fallback.
    #[test]
    fn every_route_the_router_declares_logs_one_admin_line() {
        let (journal, _guard) = capture();
        rt().block_on(async {
            let routes = declared_routes();
            assert_eq!(
                routes.len(),
                router_code().matches(".route(").count(),
                "every `.route(` in the router source must yield a path"
            );
            let admin_paths = routes
                .iter()
                .filter(|r| r.path.starts_with("/admin"))
                .count();
            assert!(
                admin_paths >= 45,
                "expected at least the 45 known /admin paths, parsed {admin_paths}"
            );
            for required in MUST_COVER {
                assert!(
                    routes.iter().any(|r| r.path == required),
                    "{required} is no longer declared under that path; the audit's coverage of it \
                     has to be re-established rather than assumed"
                );
            }

            let (base, client) = serve(test_state(AuthMode::Github)).await;
            let mut missed = Vec::new();
            let mut unrouted = Vec::new();
            for route in &routes {
                let before = journal.admin_lines().len();
                let status = call(
                    &client,
                    &base,
                    Method::TRACE,
                    &concrete(&route.path),
                    Some(ADMIN),
                )
                .await;
                if !route.any_method && status != StatusCode::METHOD_NOT_ALLOWED {
                    unrouted.push(format!("{} answered {status}", route.path));
                }
                let wrote = journal.admin_lines().len() - before;
                if wrote != 1 {
                    missed.push(format!("{} wrote {wrote} lines, want 1", route.path));
                }
            }
            assert!(
                unrouted.is_empty(),
                "these declared paths did not reach their route, so the sweep did not test them: {unrouted:#?}"
            );
            assert!(
                missed.is_empty(),
                "these routes produced no admin line: {missed:#?}"
            );
        });
    }

    /// The line itself, on a real request with its real method — the one the feedback poller
    /// makes every two minutes, which logged nothing before this.
    #[test]
    fn the_line_names_the_route_method_caller_and_status() {
        let (journal, _guard) = capture();
        rt().block_on(async {
            let (base, client) = serve(test_state(AuthMode::Github)).await;
            let status = call(
                &client,
                &base,
                Method::GET,
                "/admin/feedback?since=0&limit=50",
                Some(ADMIN),
            )
            .await;
            assert_eq!(status, StatusCode::OK);

            let lines = journal.admin_lines();
            assert_eq!(lines.len(), 1, "one line per request: {lines:#?}");
            let line = &lines[0];
            println!("SAMPLE ADMIN LOG LINE: {line}");
            for field in [
                "admin request",
                "route=/admin/feedback",
                "method=GET",
                "caller=admin",
                "status=200",
                "ts=",
            ] {
                assert!(line.contains(field), "missing {field} in: {line}");
            }
        });
    }

    /// The one way this change could make things worse: putting the credential in the journal.
    /// The relay takes the bearer from `?token=` as well as from the header, so the query string
    /// is a credential and the line carries the path alone.
    #[test]
    fn the_token_never_reaches_the_journal_even_when_it_is_in_the_query() {
        let (journal, _guard) = capture();
        rt().block_on(async {
            let (base, client) = serve(test_state(AuthMode::Github)).await;
            for path in [
                "/admin/users?token=adm_test",
                "/admin/feedback?since=0&limit=50",
            ] {
                call(&client, &base, Method::GET, path, None).await;
            }
            call(&client, &base, Method::GET, "/admin/users", Some(ADMIN)).await;

            let text = journal.text();
            assert_eq!(journal.admin_lines().len(), 2, "both admin calls logged");
            assert!(!text.contains(ADMIN), "the token is in the journal: {text}");
            assert!(
                !text.contains("since=0"),
                "the query is in the journal: {text}"
            );
            assert!(
                !text.contains("token="),
                "the query is in the journal: {text}"
            );
        });
    }

    /// The two routes a reviewer asks about by name. Neither begins `/admin`, neither logged
    /// anything before, and neither needed a line of its own: they are HTTP requests, so the
    /// layer over the router covers them the same way it covers `/admin/users`.
    #[test]
    fn the_exec_and_blob_routes_are_covered_by_the_request_layer() {
        let (journal, _guard) = capture();
        rt().block_on(async {
            let (base, client) = serve(test_state(AuthMode::Github)).await;
            call(
                &client,
                &base,
                Method::POST,
                "/projects/p_x/local/d_x/exec",
                Some(ADMIN),
            )
            .await;
            call(&client, &base, Method::GET, "/blob/p_x/k_x", Some(ADMIN)).await;
        });
        let lines = journal.admin_lines();
        assert_eq!(lines.len(), 2, "{lines:#?}");
        println!("SAMPLE EXEC LINE: {}", lines[0]);
        println!("SAMPLE BLOB LINE: {}", lines[1]);
        assert!(lines[0].contains("route=/projects/p_x/local/d_x/exec"));
        assert!(lines[0].contains("method=POST"));
        assert!(lines[1].contains("route=/blob/p_x/k_x"));
        assert!(lines[1].contains("method=GET"));
    }

    /// The socket half. `WS /room/{project}/{chat}` authenticates once and then carries every
    /// later admin action inside it, so the request layer sees the upgrade and nothing after it.
    /// These two lines are what covers the inside of the socket.
    #[test]
    fn the_room_socket_and_every_row_pushed_inside_it_are_logged() {
        let (journal, _guard) = capture();
        log_admin_socket_open(&Principal::Admin, "p_jacob/c_standup");
        // The row the relay will store and broadcast as Jacob's own device.
        log_admin_row_push(&Principal::Admin, "p_jacob/c_standup", "dev_jacob_mac", 41);
        // The same admin writing under its own name: the claim matches the principal.
        log_admin_row_push(&Principal::Admin, "p_jacob/c_standup", "admin", 42);

        let lines = journal.admin_lines();
        assert_eq!(lines.len(), 3, "{lines:#?}");
        println!("SAMPLE SOCKET LINE: {}", lines[0]);
        println!("SAMPLE ROW LINE:    {}", lines[1]);
        for field in [
            MESSAGE_SOCKET,
            "caller=admin",
            "project=p_jacob",
            "chat=c_standup",
        ] {
            assert!(lines[0].contains(field), "missing {field} in: {}", lines[0]);
        }
        for field in [
            MESSAGE_ROW,
            "caller=admin",
            "project=p_jacob",
            "chat=c_standup",
            "seq=41",
            "claimed_author=dev_jacob_mac",
            "author_verified=false",
        ] {
            assert!(lines[1].contains(field), "missing {field} in: {}", lines[1]);
        }
        assert!(
            lines[2].contains("author_verified=true"),
            "a row the admin writes under its own name is not an impersonation: {}",
            lines[2]
        );
    }

    /// No admin, no socket lines.
    #[test]
    fn a_non_admin_socket_writes_no_admin_line() {
        let (journal, _guard) = capture();
        let device = Principal::Device {
            id: "d1".into(),
            token: String::new(),
            user_id: "u1".into(),
        };
        log_admin_socket_open(&device, "p_jacob/c_standup");
        log_admin_row_push(&device, "p_jacob/c_standup", "d1", 1);
        assert!(journal.admin_lines().is_empty());
    }

    /// Records the level of every event on the audit target.
    #[derive(Clone, Default)]
    struct Levels(Arc<Mutex<Vec<tracing::Level>>>);

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Levels {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            if event.metadata().target() == TARGET {
                self.0
                    .lock()
                    .expect("levels")
                    .push(*event.metadata().level());
            }
        }
    }

    /// The trap this whole change exists to avoid. `rooms.rs` has logged joins and leaves all
    /// along, at `debug!`, and production runs `arbos_relay=info`: instrumented in code, gone
    /// before it reaches the journal, and it passed review. Pin the level so "quieten the logs"
    /// cannot repeat it here without turning CI red.
    #[test]
    fn every_admin_line_is_emitted_at_the_pinned_level() {
        let levels = Levels::default();
        let sub = tracing_subscriber::registry().with(levels.clone());
        let _guard = tracing::subscriber::set_default(sub);

        rt().block_on(async {
            let (base, client) = serve(test_state(AuthMode::Github)).await;
            call(&client, &base, Method::GET, "/admin/users", Some(ADMIN)).await;
        });
        log_admin_socket_open(&Principal::Admin, "p/c");
        log_admin_row_push(&Principal::Admin, "p/c", "d", 1);

        let seen = levels.0.lock().expect("levels").clone();
        assert_eq!(seen.len(), 3, "all three line kinds: {seen:?}");
        assert!(
            seen.iter().all(|l| *l == LEVEL),
            "admin lines must stay at {LEVEL}, got {seen:?}"
        );
        assert!(
            LEVEL <= tracing::Level::INFO,
            "`debug` and `trace` are discarded in production"
        );
    }

    /// And the other half of the same trap: the level is only useful if the filter production
    /// runs keeps it. That filter is read out of the deploy script, so dropping `arbos_relay` to
    /// `warn` there turns this test red instead of quietly emptying the audit.
    #[test]
    fn the_filter_the_deploy_script_installs_keeps_the_target() {
        let filter = RELAY_SERVER_SH
            .lines()
            .find_map(|l| l.trim().strip_prefix("RUST_LOG="))
            .expect("scripts/deploy/relay-server.sh must set RUST_LOG for the relay")
            .trim()
            .to_owned();
        assert!(
            !filter.is_empty(),
            "the deploy script's RUST_LOG is empty, so the relay would fall back to its own default"
        );

        let (journal, _guard) = {
            let journal = Journal::default();
            let sub = tracing_subscriber::registry()
                .with(tracing_subscriber::EnvFilter::new(&filter))
                .with(
                    tracing_subscriber::fmt::layer()
                        .with_ansi(false)
                        .with_writer(journal.clone()),
                );
            let guard = tracing::subscriber::set_default(sub);
            (journal, guard)
        };
        log_admin_socket_open(&Principal::Admin, "p/c");

        assert_eq!(
            journal.admin_lines().len(),
            1,
            "`RUST_LOG={filter}` from scripts/deploy/relay-server.sh drops {TARGET}; the audit \
             would be silent in production"
        );
    }

    /// Only admin requests. A device token and an unauthenticated call leave no admin line, so
    /// the admin target stays a list of what the admin did and nothing else.
    #[test]
    fn a_non_admin_caller_writes_no_admin_line() {
        let (journal, _guard) = capture();
        rt().block_on(async {
            let state = test_state(AuthMode::Github);
            let (user_id, _) = crate::people::tests_support::person(&state, 1);
            let device_token = "adv_a_real_device_token";
            state
                .db
                .insert_device(
                    "d_probe",
                    "probe",
                    &arbos_core::auth::hash(device_token),
                    &user_id,
                    "github",
                )
                .expect("device");
            let (base, client) = serve(state).await;
            for bearer in [Some(device_token), None, Some("nonsense")] {
                call(&client, &base, Method::GET, "/admin/users", bearer).await;
            }
            assert!(
                journal.admin_lines().is_empty(),
                "non-admin callers logged: {:#?}",
                journal.admin_lines()
            );
        });
    }
}

use std::collections::BTreeSet;

use arbos_core::sites::{Route, Scheme, RESERVED_PREFIXES, RESERVED_SUBDOMAINS};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Fixture {
    reserved: Vec<String>,
    routes: Vec<Case>,
}

#[derive(Debug, Deserialize)]
struct Case {
    name: String,
    scheme: String,
    labels: String,
    path: String,
    resolved: Option<Expected>,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
struct Expected {
    prefix: String,
    app: String,
    rest: String,
}

fn fixture() -> Fixture {
    serde_json::from_str(include_str!("../../../edge/sites-worker/tests/routes.json"))
        .expect("shared core/Worker route fixture")
}

#[test]
fn sites_route_contract_matches_worker_fixture() {
    for case in fixture().routes {
        let route = Route::new(
            Scheme::parse(&case.scheme).expect("fixture scheme"),
            "sites.invalid",
        );
        let got = route
            .parse_request(&format!("{}.sites.invalid", case.labels), &case.path)
            .map(|r| Expected {
                prefix: r.prefix,
                app: r.app,
                rest: r.rest,
            });
        assert_eq!(got, case.resolved, "{}", case.name);
    }
}

#[test]
fn reserved_name_fixture_is_complete_and_never_routes() {
    let fixture = fixture();
    let actual: BTreeSet<&str> = RESERVED_SUBDOMAINS
        .iter()
        .chain(RESERVED_PREFIXES.iter())
        .copied()
        .collect();
    let expected: BTreeSet<&str> = fixture.reserved.iter().map(String::as_str).collect();
    assert_eq!(
        actual, expected,
        "update the shared Worker contract for every reserved-name change"
    );
    for prefix in expected {
        for (scheme, labels) in [
            (Scheme::Subdomain, prefix.to_owned()),
            (Scheme::TwoLevel, format!("notes.{prefix}")),
        ] {
            let route = Route::new(scheme, "sites.invalid");
            assert_eq!(
                route.parse_request(&format!("{labels}.sites.invalid"), "/notes/"),
                None,
                "{prefix}"
            );
        }
    }
}

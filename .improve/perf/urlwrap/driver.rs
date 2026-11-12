use std::hint::black_box;
use std::time::Instant;

fn check_behavior() {
    let cases = [
        ("No URLs: café 🌳", "No URLs: café 🌳"),
        ("ftp://files.test/a", "ftp://files.test/a"),
        ("HTTP://example.test/a", "HTTP://example.test/a"),
        (
            "https://first.test/a http://second.test/b",
            "https:/\u{034f}/first.test\u{200b}/a http:/\u{034f}/second.test\u{200b}/b",
        ),
        (
            "http://first.test/a https://second.test/b",
            "http:/\u{034f}/first.test\u{200b}/a https:/\u{034f}/second.test\u{200b}/b",
        ),
        (
            "httpx://fake httpsomething://fake httphttps://real.test/a",
            "httpx://fake httpsomething://fake httphttps:/\u{034f}/real.test\u{200b}/a",
        ),
        (
            "mañana🌳https://éxample.test/路径?q=é&n=2",
            "mañana🌳https:/\u{034f}/éxample.test\u{200b}/路径?q=é&n=2",
        ),
        (
            "see (https://a.test/x/y), \"http://b.test/z.\" end",
            "see (https:/\u{034f}/a.test\u{200b}/x\u{200b}/y), \"http:/\u{034f}/b.test\u{200b}/z.\" end",
        ),
        (
            "https://a.test/ http://b.test/x//y",
            "https:/\u{034f}/a.test/ http:/\u{034f}/b.test\u{200b}/x\u{200b}//y",
        ),
        ("http://", "http:/\u{034f}/"),
    ];
    for (text, expected) in cases {
        let actual = urlwrap::hint_bare_urls(text);
        assert_eq!(actual, expected, "wrapping changed for {text:?}");
        assert_eq!(urlwrap::plain(&actual), text, "visible text changed");
    }
    let fragments = [
        "plain café ",
        "httpx://fake ",
        "https://a.test/path?q=1&b=2 ",
        "http://b.test/z, ",
        "🌳(https://例.test/a). ",
    ];
    let mut generated = 0;
    for first in fragments {
        for second in fragments {
            for third in fragments {
                let input = format!("{first}{second}{third}");
                assert_eq!(urlwrap::plain(&urlwrap::hint_bare_urls(&input)), input);
                generated += 1;
            }
        }
    }
    println!(
        "{{\"goldens_passed\":{},\"round_trips_passed\":{},\"failed\":0}}",
        cases.len(),
        generated
    );
}

fn benchmark(sample: usize) {
    for scheme in ["https", "http", "mixed"] {
        for links in [1, 100, 1_000] {
            let input: String = (0..links)
                .map(|i| {
                    let prefix = if scheme == "http" || scheme == "mixed" && i % 2 == 1 {
                        "http"
                    } else {
                        "https"
                    };
                    format!("{prefix}://example.test/path/{i}?q=1&b=2 ")
                })
                .collect();
            let iterations = 5_000 / links;
            for _ in 0..8 {
                black_box(urlwrap::hint_bare_urls(black_box(&input)));
            }
            let started = Instant::now();
            for _ in 0..iterations {
                black_box(urlwrap::hint_bare_urls(black_box(&input)));
            }
            let ns = started.elapsed().as_nanos() as f64 / iterations as f64;
            println!(
                "{{\"scheme\":\"{scheme}\",\"links\":{links},\"input_bytes\":{},\"sample\":{sample},\"iterations\":{iterations},\"ns_per_call\":{ns:.3}}}",
                input.len()
            );
        }
    }
}

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("bench") => benchmark(
            std::env::args()
                .nth(2)
                .expect("expected sample index")
                .parse()
                .expect("sample index must be a number"),
        ),
        Some("check") | None => check_behavior(),
        _ => panic!("expected check or bench"),
    }
}

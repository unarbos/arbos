use serde_json::{json, Value};
use std::{hint::black_box, time::Instant};
mod llm;
mod web {
    include!("web.rs");
    include!("search.rs");
    pub fn controls() -> Vec<String> {
        let mut html=vec!["".into(),"plain héllo 👋".into(),r#"<h1>API</h1><script>hidden()</script><style>.a{}</style><!-- hidden --><p>A &amp; B</p><ul><li><a href="/auth">Auth</a></li></ul>"#.into(),r##"<a href="javascript:bad()">bad</a><a href="mailto:a@b">mail</a><a href="#anchor">same</a><a href="//other.example/a">other</a>"##.into(),"&#65;&#x42; &#x110000; &#4294967296; &lt; &amp;lt;".into(),"<SVG><path>hidden</path></SVG><template>hidden</template><noscript>hidden</noscript><table><tr><th>Key</th><td>value</td></tr></table>".into(),"<li>first</li>next".into()];
        for n in 0..32 {
            html.push(format!("<h{}>Title{n}</h{}><p>héllo &#{};</p><a href=\"../a?x={n}&amp;y=2\"><b>label{n}</b></a>",n%6+1,n%6+1,65+n));
        }
        let values = html
            .iter()
            .map(|h| html_to_text(h, Some("https://docs.example/dir/page")))
            .collect::<Vec<_>>();
        assert_eq!(values[6], "- first\nnext");
        assert_eq!(values[0], "");
        assert_eq!(values[1], "plain héllo 👋");
        assert!(!values[2].contains("hidden"));
        assert!(values[2].contains("Auth (https://docs.example/auth)"));
        assert!(!values[3].contains("javascript:"));
        assert!(values[3].contains("other (https://other.example/a)"));
        assert!(!values[5].contains("hidden"));
        values
    }
    pub fn page(n: usize) -> String {
        let mut out=String::from("<html><h1>API reference</h1><script>hidden()</script><style>hidden</style><!-- hidden --><article>");
        for i in 0..n {
            out.push_str(&format!("<p>Paragraph{i} &amp; &#65; &#x1F44B;</p><a href=\"/docs/{i}?a=1&amp;b=2\"><strong>Label{i}</strong></a><ul><li>item{i}</li></ul>"));
        }
        out + "</article></html>"
    }
}
mod tree {
    include!("tree.rs");
    pub fn parse(s: &str) -> Option<(String, String, Vec<String>)> {
        parse_card(s)
    }
    pub fn controls() -> Vec<Option<(String, String, Vec<String>)>> {
        let mut inputs = vec![
            String::new(),
            "no card here".into(),
            r#"{"heading":"H","summary":"S","entities":["a",null,5]}"#.into(),
            r#"```json
{"heading":"fenced","summary":"S","entities":[]}
```"#
                .into(),
            r#"{"heading": "Only head", "summary": "cut off mid"#.into(),
            r#"prefix "heading":"héllo 👋", "summary":"escaped \"quote\" tail"#.into(),
        ];
        for n in 0..16 {
            inputs.push(format!(
                "{{\"heading\":\"{}\",\"summary\":\"{}\",\"entities\":[\"{}\"]}}",
                "é".repeat(n * 30),
                "👋".repeat(n * 120),
                "名".repeat(n * 7)
            ));
        }
        let values = inputs.iter().map(|s| parse_card(s)).collect::<Vec<_>>();
        assert!(values[0].is_none());
        assert!(values[1].is_none());
        assert_eq!(
            values[2].as_ref().unwrap(),
            &("H".into(), "S".into(), vec!["a".into()])
        );
        assert_eq!(values[4].as_ref().unwrap().1, "cut off mid");
        for value in values.iter().flatten() {
            assert!(value.0.chars().count() <= 160);
            assert!(value.1.chars().count() <= 900);
            assert!(value.2.iter().all(|e| e.chars().count() <= 40));
        }
        values
    }
}
fn main() {
    let controls =
        json!({"html":web::controls(),"cards":tree::controls(),"search":web::search_controls()});
    if std::env::args().nth(1).as_deref() != Some("bench") {
        println!("{controls}");
        return;
    }
    let page = web::page(32);
    let plain = "<p>Short &amp; readable &#65;</p>";
    let card = r#"{"heading": "Only head", "summary": "cut off mid"#;
    for _ in 0..50 {
        black_box(web::html_to_text(&page, Some("https://example/")));
        black_box(tree::parse(card));
    }
    let mut rows = serde_json::Map::new();
    for (name, input, calls) in [("html32", page.as_str(), 100), ("html1", plain, 1000)] {
        let start = Instant::now();
        for _ in 0..calls {
            black_box(web::html_to_text(
                black_box(input),
                Some("https://example/"),
            ));
        }
        rows.insert(name.into(),json!({"nanoseconds_per_call":start.elapsed().as_nanos()as f64/calls as f64,"calls":calls}));
    }
    let start = Instant::now();
    for _ in 0..2000 {
        black_box(tree::parse(black_box(card)));
    }
    rows.insert(
        "truncated_card".into(),
        json!({"nanoseconds_per_call":start.elapsed().as_nanos()as f64/2000.0,"calls":2000}),
    );
    println!(
        "{}",
        json!({"controls":controls,"timings":Value::Object(rows)})
    );
}

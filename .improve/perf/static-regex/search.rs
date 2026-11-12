pub fn search_controls() -> Vec<serde_json::Value> {
    let cases = [
        r#"<a class="result__a" href="https://a.example">A</a><a class="result__snippet">first</a>"#,
        r#"<a href="https://wrong.example" class="other">skip</a><a class="result__a" href="https://b.example/?a=1&amp;b=2"><b>héllo</b></a><a class="result__snippet">x &amp; y</a>"#,
        "none",
    ];
    cases.into_iter().map(|html| {fn captures(html:&str)->Result<serde_json::Value,String>{
/* SOURCE_SEARCH_BINDINGS */
 Ok(serde_json::json!({"links":link_re.captures_iter(html).map(|c|vec![c[1].to_string(),c[2].to_string()]).collect::<Vec<_>>(),"snippets":snippet_re.captures_iter(html).map(|c|c[1].to_string()).collect::<Vec<_>>()}))}
 captures(html).unwrap()}).collect()
}

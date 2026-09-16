//! Credential shapes scrubbed from anything that leaves the machine as a
//! feedback report: what a person could not have meant to send. The
//! kernel's own key and granted secrets are redacted by value elsewhere
//! (`arbos_engine::secrets`); this catches the ones nobody registered —
//! a token pasted into a prompt, a key in a command line, a `.env` the
//! agent read.
//!
//! Shapes, not entropy: a long hex string may be a git sha, which a report
//! needs. Each match becomes `[redacted:<kind>]`.

/// One credential shape: a name and a matcher over a token.
struct Shape {
    kind: &'static str,
    /// Prefix the token must start with (case-sensitive).
    prefix: &'static str,
    /// Least length of the whole token.
    min_len: usize,
}

const SHAPES: &[Shape] = &[
    Shape {
        kind: "openai-key",
        prefix: "sk-",
        min_len: 20,
    },
    Shape {
        kind: "anthropic-key",
        prefix: "sk-ant-",
        min_len: 20,
    },
    Shape {
        kind: "github-token",
        prefix: "ghp_",
        min_len: 20,
    },
    Shape {
        kind: "github-token",
        prefix: "gho_",
        min_len: 20,
    },
    Shape {
        kind: "github-token",
        prefix: "ghu_",
        min_len: 20,
    },
    Shape {
        kind: "github-token",
        prefix: "ghs_",
        min_len: 20,
    },
    Shape {
        kind: "github-token",
        prefix: "ghr_",
        min_len: 20,
    },
    Shape {
        kind: "github-token",
        prefix: "github_pat_",
        min_len: 30,
    },
    Shape {
        kind: "slack-token",
        prefix: "xoxb-",
        min_len: 20,
    },
    Shape {
        kind: "slack-token",
        prefix: "xoxp-",
        min_len: 20,
    },
    Shape {
        kind: "slack-token",
        prefix: "xoxa-",
        min_len: 20,
    },
    Shape {
        kind: "slack-token",
        prefix: "xoxr-",
        min_len: 20,
    },
    Shape {
        kind: "aws-key",
        prefix: "AKIA",
        min_len: 20,
    },
    Shape {
        kind: "aws-key",
        prefix: "ASIA",
        min_len: 20,
    },
    Shape {
        kind: "google-key",
        prefix: "AIza",
        min_len: 30,
    },
    Shape {
        kind: "stripe-key",
        prefix: "sk_live_",
        min_len: 20,
    },
    Shape {
        kind: "stripe-key",
        prefix: "rk_live_",
        min_len: 20,
    },
    Shape {
        kind: "vault-ref",
        prefix: "op://",
        min_len: 8,
    },
    Shape {
        kind: "jwt",
        prefix: "eyJ",
        min_len: 40,
    },
];

/// `key = value` / `key: value` / `key=value` names whose value is a
/// secret whatever it looks like.
const VALUE_KEYS: &[&str] = &[
    "api_key",
    "apikey",
    "api-key",
    "secret",
    "password",
    "passwd",
    "token",
    "authorization",
    "private_key",
    "client_secret",
    "access_key",
];

/// What `redact` did, for the report's own record.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Redacted {
    pub tokens: usize,
    pub values: usize,
    pub blocks: usize,
}

impl Redacted {
    pub fn total(&self) -> usize {
        self.tokens + self.values + self.blocks
    }
}

fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | '+' | '=' | ':' | '@')
}

/// `text` with credential shapes, secret-named values, and PEM private
/// key blocks replaced. Returns the text and a count of what went.
pub fn redact(text: &str) -> (String, Redacted) {
    let mut n = Redacted::default();
    let text = redact_pem(text, &mut n);
    let text = redact_values(&text, &mut n);
    let text = redact_tokens(&text, &mut n);
    (text, n)
}

fn redact_pem(text: &str, n: &mut Redacted) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("-----BEGIN ") {
        let Some(key_at) = rest[start..].find("PRIVATE KEY-----") else {
            break;
        };
        let body_from = start + key_at + "PRIVATE KEY-----".len();
        // Through the closing marker's own `-----`; without one, to the end.
        let end = match rest[body_from..].find("-----END ") {
            Some(e) => {
                let e = body_from + e + "-----END ".len();
                rest[e..]
                    .find("-----")
                    .map(|c| e + c + 5)
                    .unwrap_or(rest.len())
            }
            None => rest.len(),
        };
        out.push_str(&rest[..start]);
        out.push_str("[redacted:private-key]");
        n.blocks += 1;
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

fn redact_values(text: &str, n: &mut Redacted) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        out.push_str(&redact_values_line(line, n));
    }
    out
}

fn redact_values_line(line: &str, n: &mut Redacted) -> String {
    let lower = line.to_ascii_lowercase();
    let mut best: Option<(usize, usize)> = None;
    for key in VALUE_KEYS {
        let mut from = 0;
        while let Some(at) = lower[from..].find(key) {
            let at = from + at;
            let after = at + key.len();
            // The key must be a word on its own: `token` in `tokens=` or in
            // `window_tokens` is not a secret's name.
            let before_ok = at == 0
                || !lower[..at]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_ascii_alphanumeric());
            let tail = &line[after..];
            let sep = tail
                .trim_start_matches(['"', '\'', ' ', '\t'])
                .chars()
                .next();
            if before_ok && matches!(sep, Some('=') | Some(':')) {
                // Value starts after the separator and any quotes/space.
                let sep_at = after + tail.find([':', '=']).unwrap_or(0) + 1;
                let value_start = sep_at
                    + line[sep_at..]
                        .find(|c: char| !(c == ' ' || c == '\t' || c == '"' || c == '\''))
                        .unwrap_or(0);
                let value_end = value_start
                    + line[value_start..]
                        .find(|c: char| {
                            c == '"'
                                || c == '\''
                                || c == ' '
                                || c == ','
                                || c == ';'
                                || c == '\n'
                                || c == '\r'
                                || c == '&'
                        })
                        .unwrap_or(line.len() - value_start);
                if value_end > value_start && !line[value_start..value_end].starts_with("[redacted")
                {
                    best = Some((value_start, value_end));
                    break;
                }
            }
            from = after;
        }
        if best.is_some() {
            break;
        }
    }
    match best {
        Some((s, e)) => {
            n.values += 1;
            let mut out = String::new();
            out.push_str(&line[..s]);
            out.push_str("[redacted:value]");
            out.push_str(&line[e..]);
            // A line can hold more than one; go again on the rest.
            redact_values_line(&out, n)
        }
        None => line.to_string(),
    }
}

fn redact_tokens(text: &str, n: &mut Redacted) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while !rest.is_empty() {
        // Next token boundary.
        let start = match rest.find(is_token_char) {
            Some(s) => s,
            None => {
                out.push_str(rest);
                break;
            }
        };
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let end = rest.find(|c: char| !is_token_char(c)).unwrap_or(rest.len());
        let token = &rest[..end];
        let hit = SHAPES
            .iter()
            .filter(|s| token.starts_with(s.prefix) && token.len() >= s.min_len)
            .max_by_key(|s| s.prefix.len());
        match hit {
            Some(s) => {
                out.push_str("[redacted:");
                out.push_str(s.kind);
                out.push(']');
                n.tokens += 1;
            }
            None => out.push_str(token),
        }
        rest = &rest[end..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_tokens_values_and_pem_blocks_go_and_the_rest_stays() {
        let (t, n) = redact(
            "export OPENROUTER_API_KEY=sk-or-v1-0123456789abcdef0123456789abcdef && curl -H 'Authorization: Bearer ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ012345' https://x/",
        );
        assert!(!t.contains("sk-or-v1"), "{t}");
        assert!(!t.contains("ghp_ABC"), "{t}");
        assert!(t.contains("[redacted:"), "{t}");
        assert!(t.contains("curl -H") && t.contains("https://x/"), "{t}");
        assert!(n.total() >= 2, "{n:?}");

        let (t, n) = redact(
            "api_key = \"abc12345\"\nmodel = \"anthropic/claude-opus-5\"\nwindow_tokens = 32000\n",
        );
        assert_eq!(
            t,
            "api_key = \"[redacted:value]\"\nmodel = \"anthropic/claude-opus-5\"\nwindow_tokens = 32000\n"
        );
        assert_eq!(n.values, 1);

        let pem = "before\n-----BEGIN RSA PRIVATE KEY-----\nMIIEow\nAAAA\n-----END RSA PRIVATE KEY-----\nafter";
        let (t, n) = redact(pem);
        assert_eq!(t, "before\n[redacted:private-key]\nafter");
        assert_eq!(n.blocks, 1);
        // As a JSON string value: one line, escaped newlines; what follows
        // the block on the line stays.
        let (t, _) = redact(
            r#"{"body":"-----BEGIN PRIVATE KEY-----\nMIIE\n-----END PRIVATE KEY-----","next":1}"#,
        );
        assert_eq!(t, r#"{"body":"[redacted:private-key]","next":1}"#);

        // A git sha, a URL, a model id, a path: kept.
        let plain = "commit 4b387fc0e1 on main; see https://github.com/unarbos/arbos/pull/320 and /home/jacob/.arbos/agents/root; op://vault/item stays? no";
        let (t, _) = redact(plain);
        assert!(
            t.contains("4b387fc0e1")
                && t.contains("pull/320")
                && t.contains("/home/jacob/.arbos/agents/root"),
            "{t}"
        );
        assert!(t.contains("[redacted:vault-ref]"), "{t}");
        let (t, n) = redact("The token count was 37k tokens; tokens=12");
        assert_eq!(n.total(), 0, "{t}");
    }
}

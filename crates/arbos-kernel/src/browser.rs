use anyhow::{Result, bail};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    process::{Child, Command, Stdio},
    sync::Mutex,
};

/// What one browser action gives back: text for the model, and for
/// `screenshot` the PNG bytes, which the tool writes to a file so the
/// projection can send them as an image part.
pub struct BrowserOut {
    pub text: String,
    pub png: Option<Vec<u8>>,
}

impl From<String> for BrowserOut {
    fn from(text: String) -> Self {
        Self { text, png: None }
    }
}

/// A Chromium page. Chrome is started once per kernel if present.
pub struct BrowserHub {
    chrome: Mutex<Option<Child>>,
    port: Mutex<Option<u16>>,
    /// Chrome's own profile dir for this kernel; removed when the hub drops.
    profile: Mutex<Option<std::path::PathBuf>>,
    pages: Mutex<HashMap<String, String>>,
}

impl BrowserHub {
    pub fn new() -> Self {
        Self {
            chrome: Mutex::new(None),
            port: Mutex::new(None),
            profile: Mutex::new(None),
            pages: Mutex::new(HashMap::new()),
        }
    }

    /// The page's current URL, empty before the first navigate.
    pub fn url(&self, agent: &str) -> String {
        self.pages
            .lock()
            .unwrap()
            .get(agent)
            .cloned()
            .unwrap_or_default()
    }

    /// Register the agent's page. True the first time — the caller opens
    /// the desktop row then.
    pub fn touch(&self, agent: &str) -> bool {
        let mut pages = self.pages.lock().unwrap();
        if pages.contains_key(agent) {
            return false;
        }
        pages.insert(agent.to_string(), String::new());
        true
    }

    /// Forget the agent's page. The next action starts a fresh one.
    pub fn close(&self, agent: &str) -> bool {
        self.pages.lock().unwrap().remove(agent).is_some()
    }

    pub fn act(&self, agent: &str, action: &str, args: &Value) -> Result<BrowserOut> {
        match action {
            "navigate" => {
                let url = args
                    .get("url")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("url required"))?;
                self.pages
                    .lock()
                    .unwrap()
                    .insert(agent.to_string(), url.to_string());
                match self.session() {
                    Ok(mut cdp) => {
                        cdp.navigate(url)?;
                        let title = cdp.title().unwrap_or_default();
                        let text = cdp.snapshot().unwrap_or_default();
                        Ok(format!("opened {url}\ntitle: {title}\n{text}").into())
                    }
                    // No Chrome on this machine: a plain fetch still answers
                    // "what is on that page", read-only.
                    Err(err) => match fetch_blocking(url) {
                        Some(html) => {
                            Ok(format!("(no chrome: {err}; fetched over HTTP)\n{html}").into())
                        }
                        None => Err(err),
                    },
                }
            }
            "screenshot" => {
                let mut cdp = self.session()?;
                let png = cdp.screenshot()?;
                Ok(BrowserOut {
                    text: "screenshot taken".to_string(),
                    png: Some(png),
                })
            }
            "snapshot" => match self.session() {
                Ok(mut cdp) => Ok(cdp.snapshot()?.into()),
                Err(err) => {
                    let url = self.url(agent);
                    Ok(fetch_blocking(&url)
                        .map(|html| format!("(no chrome: {err}; fetched over HTTP)\n{html}"))
                        .unwrap_or_else(|| format!("url: {url}"))
                        .into())
                }
            },
            "click" => {
                let r#ref = args
                    .get("ref")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("click needs ref (from snapshot)"))?;
                let mut cdp = self.session()?;
                cdp.click(r#ref)?;
                // The click may have navigated; report where the page is now.
                let url = cdp.url().unwrap_or_default();
                if !url.is_empty() {
                    self.pages
                        .lock()
                        .unwrap()
                        .insert(agent.to_string(), url.clone());
                }
                Ok(format!("clicked {ref}\n{}", cdp.snapshot().unwrap_or_default()).into())
            }
            "type" => {
                let r#ref = args
                    .get("ref")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("type needs ref (from snapshot)"))?;
                let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
                let mut cdp = self.session()?;
                cdp.type_into(r#ref, text)?;
                Ok(format!("typed into {ref}").into())
            }
            other => bail!("unknown browser action {other}"),
        }
    }

    /// A picture of the page for the desktop's browser row, taken after an
    /// action that changed it. Not for the model: nothing is attached to the
    /// tool result, so the transcript does not pay for it.
    pub fn preview(&self) -> Option<Vec<u8>> {
        let port = *self.port.lock().unwrap();
        let mut cdp = Cdp::connect(port?).ok()?;
        cdp.screenshot().ok()
    }

    /// A CDP session on the kernel's one page, starting Chrome if needed.
    fn session(&self) -> Result<Cdp> {
        let port = self.ensure_chrome()?;
        Cdp::connect(port)
    }

    fn ensure_chrome(&self) -> Result<u16> {
        if let Some(p) = *self.port.lock().unwrap() {
            return Ok(p);
        }
        let bin = which::which("chromium")
            .or_else(|_| which::which("google-chrome"))
            .or_else(|_| which::which("chromium-browser"))
            .or_else(|_| {
                which::which("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome")
            })?;
        // Port 0 lets Chrome pick a free one, so two kernels on one machine
        // (two places open, a test run beside the desktop) never fight over
        // 9222 and quietly drive each other's browser. Chrome writes the port
        // it chose to DevToolsActivePort in its profile dir; the profile is
        // this process's own, under the temp dir, and is removed on exit.
        let profile = std::env::temp_dir().join(format!("arbos-chrome-{}", std::process::id()));
        std::fs::create_dir_all(&profile)?;
        let _ = std::fs::remove_file(profile.join("DevToolsActivePort"));
        let child = Command::new(bin)
            .args([
                "--headless=new",
                "--disable-gpu",
                "--no-first-run",
                "--remote-debugging-port=0",
                "--remote-allow-origins=*",
            ])
            .arg(format!("--user-data-dir={}", profile.display()))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        *self.chrome.lock().unwrap() = Some(child);
        // Chrome takes a moment to open the devtools port; poll for the file,
        // then for the port to answer, and fail with a clear message.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
        let port = loop {
            let announced = std::fs::read_to_string(profile.join("DevToolsActivePort"))
                .ok()
                .and_then(|text| text.lines().next()?.trim().parse::<u16>().ok());
            if let Some(port) = announced
                && reqwest::blocking::get(format!("http://127.0.0.1:{port}/json/version")).is_ok()
            {
                break port;
            }
            if std::time::Instant::now() > deadline {
                bail!("chrome started but never announced a devtools port");
            }
            std::thread::sleep(std::time::Duration::from_millis(150));
        };
        *self.profile.lock().unwrap() = Some(profile);
        *self.port.lock().unwrap() = Some(port);
        Ok(port)
    }
}

impl Drop for BrowserHub {
    fn drop(&mut self) {
        if let Some(mut child) = self.chrome.lock().unwrap().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(profile) = self.profile.lock().unwrap().take() {
            let _ = std::fs::remove_dir_all(profile);
        }
    }
}

impl Default for BrowserHub {
    fn default() -> Self {
        Self::new()
    }
}

fn fetch_blocking(url: &str) -> Option<String> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return None;
    }
    reqwest::blocking::Client::new()
        .get(url)
        .send()
        .ok()
        .and_then(|r| r.text().ok())
}

/// One WebSocket to one page. Every action opens a fresh session: Chrome
/// keeps the page, so nothing is lost between calls, and a dropped socket
/// never wedges the kernel.
struct Cdp {
    socket: tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<std::net::TcpStream>>,
    next_id: u64,
}

/// Enumerate the page for the model: interactive elements get a stable
/// `data-arbos-ref` so `click`/`type` can name them, and the visible text
/// comes back trimmed.
const SNAPSHOT_JS: &str = r#"(() => {
  const picks = Array.from(document.querySelectorAll('a[href], button, input, textarea, select, [role="button"], [role="link"], [onclick]'));
  let n = 0;
  const rows = [];
  for (const el of picks) {
    if (!(el.offsetWidth || el.offsetHeight || el.getClientRects().length)) continue;
    n += 1;
    el.setAttribute('data-arbos-ref', String(n));
    const tag = el.tagName.toLowerCase();
    const label = (el.getAttribute('aria-label') || el.innerText || el.value || el.placeholder || el.getAttribute('href') || '').trim().replace(/\s+/g, ' ').slice(0, 80);
    const kind = tag === 'input' ? `input[${el.type || 'text'}]` : tag;
    rows.push(`[${n}] ${kind} ${label}`);
    if (rows.length >= 120) break;
  }
  const text = (document.body ? document.body.innerText : '').replace(/\n{3,}/g, '\n\n').slice(0, 4000);
  return `url: ${location.href}\ntitle: ${document.title}\n\n${text}\n\ninteractive:\n${rows.join('\n')}`;
})()"#;

impl Cdp {
    fn connect(port: u16) -> Result<Self> {
        let list = reqwest::blocking::get(format!("http://127.0.0.1:{port}/json/list"))?.text()?;
        let pages: Value = serde_json::from_str(&list).unwrap_or(json!([]));
        let mut ws = pages
            .as_array()
            .into_iter()
            .flatten()
            .find(|p| p.get("type").and_then(Value::as_str) == Some("page"))
            .and_then(|p| p.get("webSocketDebuggerUrl"))
            .and_then(Value::as_str)
            .map(str::to_string);
        if ws.is_none() {
            // Chrome opened with no page (or the model closed it): make one.
            let made = reqwest::blocking::Client::new()
                .put(format!("http://127.0.0.1:{port}/json/new?about:blank"))
                .send()?
                .text()?;
            ws = serde_json::from_str::<Value>(&made).ok().and_then(|p| {
                p.get("webSocketDebuggerUrl")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            });
        }
        let ws = ws.ok_or_else(|| anyhow::anyhow!("chrome has no page to attach to"))?;
        let (socket, _) = tungstenite::connect(&ws)?;
        let mut cdp = Self { socket, next_id: 0 };
        cdp.set_timeout(std::time::Duration::from_secs(15))?;
        Ok(cdp)
    }

    fn set_timeout(&mut self, dur: std::time::Duration) -> Result<()> {
        if let tungstenite::stream::MaybeTlsStream::Plain(stream) = self.socket.get_ref() {
            stream.set_read_timeout(Some(dur))?;
        }
        Ok(())
    }

    /// Send `method` and wait for its reply, skipping events. Events are
    /// returned to callers that ask for one by name via `wait_event`.
    fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        self.next_id += 1;
        let id = self.next_id;
        let msg = json!({"id": id, "method": method, "params": params}).to_string();
        self.socket.send(tungstenite::Message::Text(msg.into()))?;
        loop {
            let reply = self.socket.read()?;
            let tungstenite::Message::Text(text) = reply else {
                continue;
            };
            let v: Value = serde_json::from_str(&text)?;
            if v.get("id").and_then(Value::as_u64) == Some(id) {
                if let Some(err) = v.get("error") {
                    bail!(
                        "{method}: {}",
                        err.get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("error")
                    );
                }
                return Ok(v.get("result").cloned().unwrap_or(Value::Null));
            }
        }
    }

    fn wait_event(&mut self, name: &str, budget: std::time::Duration) -> bool {
        let deadline = std::time::Instant::now() + budget;
        while std::time::Instant::now() < deadline {
            let Ok(reply) = self.socket.read() else {
                return false;
            };
            if let tungstenite::Message::Text(text) = reply
                && let Ok(v) = serde_json::from_str::<Value>(&text)
                && v.get("method").and_then(Value::as_str) == Some(name)
            {
                return true;
            }
        }
        false
    }

    fn navigate(&mut self, url: &str) -> Result<()> {
        self.call("Page.enable", json!({}))?;
        let result = self.call("Page.navigate", json!({"url": url}))?;
        if let Some(err) = result.get("errorText").and_then(Value::as_str) {
            bail!("navigate {url}: {err}");
        }
        // Give the load event a few seconds; a slow page still answers.
        self.set_timeout(std::time::Duration::from_secs(3))?;
        self.wait_event("Page.loadEventFired", std::time::Duration::from_secs(8));
        self.set_timeout(std::time::Duration::from_secs(15))?;
        Ok(())
    }

    fn eval(&mut self, expression: &str) -> Result<Value> {
        let result = self.call(
            "Runtime.evaluate",
            json!({"expression": expression, "returnByValue": true, "awaitPromise": true}),
        )?;
        if let Some(ex) = result.get("exceptionDetails") {
            bail!(
                "page script failed: {}",
                ex.pointer("/exception/description")
                    .and_then(Value::as_str)
                    .unwrap_or("exception")
            );
        }
        Ok(result
            .pointer("/result/value")
            .cloned()
            .unwrap_or(Value::Null))
    }

    fn title(&mut self) -> Result<String> {
        Ok(self
            .eval("document.title")?
            .as_str()
            .unwrap_or("")
            .to_string())
    }

    fn url(&mut self) -> Result<String> {
        Ok(self
            .eval("location.href")?
            .as_str()
            .unwrap_or("")
            .to_string())
    }

    fn snapshot(&mut self) -> Result<String> {
        Ok(self.eval(SNAPSHOT_JS)?.as_str().unwrap_or("").to_string())
    }

    fn screenshot(&mut self) -> Result<Vec<u8>> {
        let result = self.call("Page.captureScreenshot", json!({"format": "png"}))?;
        let b64 = result
            .get("data")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("screenshot: no image data"))?;
        Ok(base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            b64,
        )?)
    }

    fn click(&mut self, r#ref: &str) -> Result<()> {
        let n: u64 = r#ref
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse()
            .map_err(|_| anyhow::anyhow!("ref must be a number from snapshot, got {ref:?}"))?;
        let js = format!(
            r#"(() => {{ const el = document.querySelector('[data-arbos-ref="{n}"]'); if (!el) return 'missing'; el.scrollIntoView({{block: 'center'}}); el.click(); return 'ok'; }})()"#
        );
        match self.eval(&js)?.as_str() {
            Some("ok") => {
                // A navigation may follow; let it settle briefly.
                self.set_timeout(std::time::Duration::from_secs(2))?;
                self.wait_event("Page.loadEventFired", std::time::Duration::from_secs(2));
                self.set_timeout(std::time::Duration::from_secs(15))?;
                Ok(())
            }
            _ => bail!("no element with ref {n}; take a new snapshot"),
        }
    }

    fn type_into(&mut self, r#ref: &str, text: &str) -> Result<()> {
        let n: u64 = r#ref
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse()
            .map_err(|_| anyhow::anyhow!("ref must be a number from snapshot, got {ref:?}"))?;
        let js = format!(
            r#"(() => {{ const el = document.querySelector('[data-arbos-ref="{n}"]'); if (!el) return 'missing'; el.focus(); return 'ok'; }})()"#
        );
        if self.eval(&js)?.as_str() != Some("ok") {
            bail!("no element with ref {n}; take a new snapshot");
        }
        // Real key input, so frameworks see the same events a person makes.
        self.call("Input.insertText", json!({"text": text}))?;
        Ok(())
    }
}

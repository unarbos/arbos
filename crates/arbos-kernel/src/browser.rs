use anyhow::{Result, bail};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::Path,
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
            "fill" => {
                // Clear, then type: a form field that already holds text.
                let r#ref = args
                    .get("ref")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("fill needs ref (from snapshot)"))?;
                let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
                let mut cdp = self.session()?;
                cdp.fill(r#ref, text)?;
                Ok(format!("filled {ref}").into())
            }
            "press" => {
                // A key on the focused element: Enter to submit, Tab, Escape,
                // ArrowDown, or one character.
                let key = args.get("key").and_then(|v| v.as_str()).ok_or_else(|| {
                    anyhow::anyhow!("press needs key (Enter, Tab, Escape, ArrowDown, a)")
                })?;
                let mut cdp = self.session()?;
                cdp.press(key)?;
                let url = cdp.url().unwrap_or_default();
                if !url.is_empty() {
                    self.pages.lock().unwrap().insert(agent.to_string(), url);
                }
                Ok(format!(
                    "pressed {key}
{}",
                    cdp.snapshot().unwrap_or_default()
                )
                .into())
            }
            "hover" => {
                let r#ref = args
                    .get("ref")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("hover needs ref (from snapshot)"))?;
                let mut cdp = self.session()?;
                cdp.hover(r#ref)?;
                Ok(format!(
                    "hovering {ref}
{}",
                    cdp.snapshot().unwrap_or_default()
                )
                .into())
            }
            "select" => {
                // A <select>: the option by its visible text or value.
                let r#ref = args
                    .get("ref")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("select needs ref (from snapshot)"))?;
                let value = args
                    .get("value")
                    .or_else(|| args.get("text"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        anyhow::anyhow!("select needs value (an option's text or value)")
                    })?;
                let mut cdp = self.session()?;
                let picked = cdp.select(r#ref, value)?;
                Ok(format!("selected {picked} in {ref}").into())
            }
            "scroll" => {
                // By a ref (into view) or by direction: down (default), up,
                // top, bottom; `amount` pixels, default one screen.
                let mut cdp = self.session()?;
                let where_ = cdp.scroll(
                    args.get("ref").and_then(|v| v.as_str()),
                    args.get("direction")
                        .and_then(|v| v.as_str())
                        .unwrap_or("down"),
                    args.get("amount").and_then(number),
                )?;
                Ok(format!(
                    "{where_}
{}",
                    cdp.snapshot().unwrap_or_default()
                )
                .into())
            }
            "back" | "forward" => {
                let mut cdp = self.session()?;
                cdp.history(action)?;
                let url = cdp.url().unwrap_or_default();
                if !url.is_empty() {
                    self.pages.lock().unwrap().insert(agent.to_string(), url);
                }
                Ok(format!(
                    "went {action}
{}",
                    cdp.snapshot().unwrap_or_default()
                )
                .into())
            }
            "wait" => {
                // Until text appears on the page, or a ref exists, or `ms`
                // pass; whichever is given. Never longer than 30 s.
                let mut cdp = self.session()?;
                let text = args.get("text").and_then(|v| v.as_str());
                let r#ref = args.get("ref").and_then(|v| v.as_str());
                let ms = args
                    .get("ms")
                    .and_then(number)
                    .map(|n| n.max(0.0) as u64)
                    .unwrap_or(5_000)
                    .min(30_000);
                let outcome = cdp.wait_for(text, r#ref, ms)?;
                Ok(format!(
                    "{outcome}
{}",
                    cdp.snapshot().unwrap_or_default()
                )
                .into())
            }
            "eval" => {
                // A JavaScript expression in the page; its value comes back
                // as JSON. For reading state the snapshot does not show.
                let expression = args
                    .get("expression")
                    .or_else(|| args.get("script"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("eval needs expression"))?;
                let mut cdp = self.session()?;
                let value = cdp.eval(expression)?;
                let text = serde_json::to_string_pretty(&value).unwrap_or_default();
                Ok(arbos_core::text::clip(&text, 8_000).into())
            }
            "console" => {
                // Console messages and uncaught errors since the page loaded,
                // gathered by a hook installed the first time it is asked for.
                let mut cdp = self.session()?;
                Ok(cdp.console()?.into())
            }
            other => bail!(
                "unknown browser action {other}; use navigate, snapshot, screenshot, click, type, fill, press, hover, select, scroll, back, forward, wait, eval, console, close"
            ),
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

    /// Draw a local HTML file as a PNG of `width`×`height` CSS pixels in
    /// a page of its own, then close that page: the agents' browser pages
    /// are left as they are. This is the kernel's running Chrome over CDP
    /// — the one path that works on every machine the tests run on; a
    /// one-shot `chrome --screenshot` sat for a minute on the CI runner.
    pub fn render_file(&self, html: &Path, width: u32, height: u32) -> Result<Vec<u8>> {
        let port = self.ensure_chrome()?;
        let (mut cdp, target) = Cdp::connect_new(port)?;
        let shot = (|| {
            cdp.call(
                "Emulation.setDeviceMetricsOverride",
                json!({"width": width, "height": height, "deviceScaleFactor": 1, "mobile": false}),
            )?;
            cdp.navigate(&format!("file://{}", html.display()))?;
            cdp.screenshot()
        })();
        // The page goes whatever happened; a failed close is not the
        // caller's problem.
        let _ = reqwest::blocking::get(format!("http://127.0.0.1:{port}/json/close/{target}"));
        shot
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
                // CI runners and containers: no user namespaces for the
                // sandbox, a tiny /dev/shm. Headless Chrome is fine without.
                "--no-sandbox",
                "--disable-dev-shm-usage",
            ])
            .arg(format!("--user-data-dir={}", profile.display()))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        *self.chrome.lock().unwrap() = Some(child);
        // Chrome takes a moment to open the devtools port; poll for the file,
        // then for the port to answer, and fail with a clear message.
        // A cold runner takes a while to bring Chrome up.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(25);
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

/// The console hook alone, for `Page.addScriptToEvaluateOnNewDocument`.
const CONSOLE_INSTALL_JS: &str = r#"(() => {
  if (!window.__arbosConsole) {
    const keep = [];
    window.__arbosConsole = keep;
    const push = (level, args) => { try { keep.push(level + ': ' + Array.from(args).map(a => { try { return typeof a === 'string' ? a : JSON.stringify(a); } catch (e) { return String(a); } }).join(' ').slice(0, 500)); } catch (e) {} if (keep.length > 200) keep.shift(); };
    for (const level of ['log', 'info', 'warn', 'error', 'debug']) {
      const orig = console[level];
      console[level] = function () { push(level, arguments); return orig && orig.apply(console, arguments); };
    }
    window.addEventListener('error', e => push('uncaught', [e.message + ' @ ' + e.filename + ':' + e.lineno]));
    window.addEventListener('unhandledrejection', e => push('unhandledrejection', [String(e.reason)]));
  }
})()"#;

/// Install (once) a hook that keeps console output and errors, and return
/// what it has.
const CONSOLE_JS: &str = r#"(() => {
  if (!window.__arbosConsole) {
    const keep = [];
    window.__arbosConsole = keep;
    const push = (level, args) => { try { keep.push(level + ': ' + Array.from(args).map(a => { try { return typeof a === 'string' ? a : JSON.stringify(a); } catch (e) { return String(a); } }).join(' ').slice(0, 500)); } catch (e) {} if (keep.length > 200) keep.shift(); };
    for (const level of ['log', 'info', 'warn', 'error', 'debug']) {
      const orig = console[level];
      console[level] = function () { push(level, arguments); return orig && orig.apply(console, arguments); };
    }
    window.addEventListener('error', e => push('uncaught', [e.message + ' @ ' + e.filename + ':' + e.lineno]));
    window.addEventListener('unhandledrejection', e => push('unhandledrejection', [String(e.reason)]));
  }
  return window.__arbosConsole.slice();
})()"#;

/// A number the model sent as a number or as a string.
fn number(v: &Value) -> Option<f64> {
    v.as_f64().or_else(|| v.as_str()?.trim().parse().ok())
}

/// `[12]` or `12` → 12.
fn ref_number(r#ref: &str) -> Result<u64> {
    r#ref
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .parse()
        .map_err(|_| anyhow::anyhow!("ref must be a number from snapshot, got {ref:?}"))
}

/// DOM `key`, `code`, and Windows virtual key for the names a model uses.
fn key_codes(key: &str) -> (String, String, u32) {
    let k = key.trim();
    let lower = k.to_ascii_lowercase();
    let (dom, code, win) = match lower.as_str() {
        "enter" | "return" => ("Enter", "Enter", 13),
        "tab" => ("Tab", "Tab", 9),
        "escape" | "esc" => ("Escape", "Escape", 27),
        "backspace" => ("Backspace", "Backspace", 8),
        "delete" => ("Delete", "Delete", 46),
        "space" | " " => (" ", "Space", 32),
        "arrowdown" | "down" => ("ArrowDown", "ArrowDown", 40),
        "arrowup" | "up" => ("ArrowUp", "ArrowUp", 38),
        "arrowleft" | "left" => ("ArrowLeft", "ArrowLeft", 37),
        "arrowright" | "right" => ("ArrowRight", "ArrowRight", 39),
        "home" => ("Home", "Home", 36),
        "end" => ("End", "End", 35),
        "pageup" => ("PageUp", "PageUp", 33),
        "pagedown" => ("PageDown", "PageDown", 34),
        _ => {
            let c = k.chars().next().unwrap_or(' ');
            let win = c.to_ascii_uppercase() as u32;
            return (c.to_string(), format!("Key{}", c.to_ascii_uppercase()), win);
        }
    };
    (dom.to_string(), code.to_string(), win)
}

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

    /// A fresh `about:blank` page of its own (its target id comes back for
    /// closing it), so a render never touches an agent's page.
    fn connect_new(port: u16) -> Result<(Self, String)> {
        let made = reqwest::blocking::Client::new()
            .put(format!("http://127.0.0.1:{port}/json/new?about:blank"))
            .send()?
            .text()?;
        let page: Value = serde_json::from_str(&made)?;
        let ws = page
            .get("webSocketDebuggerUrl")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("chrome opened no page to draw in"))?;
        let id = page
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let (socket, _) = tungstenite::connect(ws)?;
        let mut cdp = Self { socket, next_id: 0 };
        cdp.set_timeout(std::time::Duration::from_secs(15))?;
        Ok((cdp, id))
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
        // The console hook goes in before the page's own scripts run, so
        // `console` sees everything from load on. Once per target; Chrome
        // keeps it across navigations.
        let _ = self.call(
            "Page.addScriptToEvaluateOnNewDocument",
            json!({"source": CONSOLE_INSTALL_JS}),
        );
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
        let n = ref_number(r#ref)?;
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

    /// Focus `ref` and clear it: the element itself for inputs and text
    /// areas, the contenteditable's text otherwise.
    fn focus_clear(&mut self, r#ref: &str) -> Result<()> {
        let n = ref_number(r#ref)?;
        let js = format!(
            r#"(() => {{ const el = document.querySelector('[data-arbos-ref="{n}"]'); if (!el) return 'missing'; el.focus(); if ('value' in el) {{ el.value = ''; el.dispatchEvent(new Event('input', {{bubbles: true}})); }} else if (el.isContentEditable) {{ el.textContent = ''; }} return 'ok'; }})()"#
        );
        if self.eval(&js)?.as_str() != Some("ok") {
            bail!("no element with ref {n}; take a new snapshot");
        }
        Ok(())
    }

    fn fill(&mut self, r#ref: &str, text: &str) -> Result<()> {
        self.focus_clear(r#ref)?;
        self.call("Input.insertText", json!({"text": text}))?;
        Ok(())
    }

    /// One key, as a person presses it: keydown, keyup, and for a single
    /// printable character the text too. Names follow the DOM `key` values.
    fn press(&mut self, key: &str) -> Result<()> {
        let (dom_key, code, win) = key_codes(key);
        let mut down =
            json!({"type": "keyDown", "key": dom_key, "code": code, "windowsVirtualKeyCode": win});
        if dom_key.chars().count() == 1 {
            down["text"] = json!(dom_key);
        } else if dom_key == "Enter" {
            // Without the text Chrome treats it as a raw key: no implicit
            // form submission, no newline.
            down["text"] = json!("\r");
            down["unmodifiedText"] = json!("\r");
        }
        self.call("Input.dispatchKeyEvent", down)?;
        self.call(
            "Input.dispatchKeyEvent",
            json!({"type": "keyUp", "key": dom_key, "code": code, "windowsVirtualKeyCode": win}),
        )?;
        if dom_key == "Enter" {
            self.set_timeout(std::time::Duration::from_secs(2))?;
            self.wait_event("Page.loadEventFired", std::time::Duration::from_secs(2));
            self.set_timeout(std::time::Duration::from_secs(15))?;
        }
        Ok(())
    }

    fn hover(&mut self, r#ref: &str) -> Result<()> {
        let n = ref_number(r#ref)?;
        let js = format!(
            r#"(() => {{ const el = document.querySelector('[data-arbos-ref="{n}"]'); if (!el) return null; el.scrollIntoView({{block: 'center'}}); const r = el.getBoundingClientRect(); return [r.left + r.width / 2, r.top + r.height / 2]; }})()"#
        );
        let point = self.eval(&js)?;
        let (x, y) = match point.as_array().map(|a| (a[0].as_f64(), a[1].as_f64())) {
            Some((Some(x), Some(y))) => (x, y),
            _ => bail!("no element with ref {n}; take a new snapshot"),
        };
        self.call(
            "Input.dispatchMouseEvent",
            json!({"type": "mouseMoved", "x": x, "y": y}),
        )?;
        std::thread::sleep(std::time::Duration::from_millis(150));
        Ok(())
    }

    fn select(&mut self, r#ref: &str, value: &str) -> Result<String> {
        let n = ref_number(r#ref)?;
        let wanted = serde_json::to_string(value)?;
        let js = format!(
            r#"(() => {{ const el = document.querySelector('[data-arbos-ref="{n}"]'); if (!el) return 'missing'; if (el.tagName !== 'SELECT') return 'not-select'; const want = {wanted}; const opt = Array.from(el.options).find(o => o.value === want || o.text.trim() === want.trim()); if (!opt) return 'no-option:' + Array.from(el.options).map(o => o.text.trim()).join(' | '); el.value = opt.value; el.dispatchEvent(new Event('input', {{bubbles: true}})); el.dispatchEvent(new Event('change', {{bubbles: true}})); return 'ok:' + opt.text.trim(); }})()"#
        );
        match self.eval(&js)?.as_str().unwrap_or("") {
            "missing" => bail!("no element with ref {n}; take a new snapshot"),
            "not-select" => bail!("ref {n} is not a <select>; use click or type"),
            s if s.starts_with("no-option:") => bail!(
                "no option {value:?} in ref {n}; options: {}",
                &s["no-option:".len()..]
            ),
            s => Ok(s.trim_start_matches("ok:").to_string()),
        }
    }

    fn scroll(
        &mut self,
        r#ref: Option<&str>,
        direction: &str,
        amount: Option<f64>,
    ) -> Result<String> {
        if let Some(r) = r#ref {
            let n = ref_number(r)?;
            let js = format!(
                r#"(() => {{ const el = document.querySelector('[data-arbos-ref="{n}"]'); if (!el) return 'missing'; el.scrollIntoView({{block: 'center'}}); return 'ok'; }})()"#
            );
            if self.eval(&js)?.as_str() != Some("ok") {
                bail!("no element with ref {n}; take a new snapshot");
            }
            return Ok(format!("scrolled to {r}"));
        }
        // Default: most of one screen, so nothing is skipped.
        let step = match amount {
            Some(px) if px > 0.0 => format!("{px}"),
            _ => "window.innerHeight * 0.9".to_string(),
        };
        let js = match direction {
            "top" => "window.scrollTo(0, 0)".to_string(),
            "bottom" => "window.scrollTo(0, document.body.scrollHeight)".to_string(),
            "up" => format!("window.scrollBy(0, -({step}))"),
            "down" => format!("window.scrollBy(0, {step})"),
            other => bail!("scroll direction must be up, down, top, or bottom, not {other:?}"),
        };
        self.eval(&js)?;
        std::thread::sleep(std::time::Duration::from_millis(120));
        let at = self.eval("Math.round(window.scrollY) + '/' + Math.round(document.body.scrollHeight - window.innerHeight)")?;
        Ok(format!(
            "scrolled {direction} (at {} px)",
            at.as_str().unwrap_or("?")
        ))
    }

    fn history(&mut self, which: &str) -> Result<()> {
        let js = if which == "back" {
            "history.back()"
        } else {
            "history.forward()"
        };
        self.eval(js)?;
        self.set_timeout(std::time::Duration::from_secs(3))?;
        self.wait_event("Page.loadEventFired", std::time::Duration::from_secs(3));
        self.set_timeout(std::time::Duration::from_secs(15))?;
        Ok(())
    }

    /// Poll the page until `text` is on it, `ref` exists, or `ms` pass.
    fn wait_for(&mut self, text: Option<&str>, r#ref: Option<&str>, ms: u64) -> Result<String> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(ms);
        let n = r#ref.map(ref_number).transpose()?;
        loop {
            let found = match (text, n) {
                (Some(t), _) => {
                    let want = serde_json::to_string(t)?;
                    self.eval(&format!(
                        "(document.body ? document.body.innerText : '').includes({want})"
                    ))?
                    .as_bool()
                    .unwrap_or(false)
                }
                (None, Some(n)) => self
                    .eval(&format!(
                        r#"!!document.querySelector('[data-arbos-ref="{n}"]')"#
                    ))?
                    .as_bool()
                    .unwrap_or(false),
                (None, None) => false,
            };
            if found {
                return Ok(match (text, r#ref) {
                    (Some(t), _) => format!("found {t:?}"),
                    (_, Some(r)) => format!("found {r}"),
                    _ => "waited".to_string(),
                });
            }
            if std::time::Instant::now() >= deadline {
                return Ok(match (text, r#ref) {
                    (Some(t), _) => format!("waited {ms} ms; {t:?} is not on the page"),
                    (_, Some(r)) => format!("waited {ms} ms; {r} did not appear"),
                    _ => format!("waited {ms} ms"),
                });
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
    }

    /// Console messages and uncaught errors, newest last. A hook on the
    /// page keeps the last 200 since it was installed (the first call
    /// installs it; nothing from before is known).
    fn console(&mut self) -> Result<String> {
        let out = self.eval(CONSOLE_JS)?;
        let lines: Vec<String> = out
            .as_array()
            .map(|rows| {
                rows.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        if lines.is_empty() {
            return Ok(
                "console: nothing since the hook was installed (call again after the next action)"
                    .to_string(),
            );
        }
        Ok(format!(
            "console ({} lines):\n{}",
            lines.len(),
            lines.join("\n")
        ))
    }

    fn type_into(&mut self, r#ref: &str, text: &str) -> Result<()> {
        let n = ref_number(r#ref)?;
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

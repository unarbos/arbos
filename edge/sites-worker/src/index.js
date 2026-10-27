// Agent-hosted sites at the edge (design #2060). One Worker on `*.<DOMAIN>/*` serves every
// published site from R2: `sites/<prefix>/<app>/manifest.json` (path → blob sha, content type,
// status) and `blobs/<sha>` (immutable, content-addressed). No box and no relay host is in the
// path. The relay writes the objects; this Worker only reads.
//
// Routing mirrors `arbos_core::sites::Route` for the deployed SCHEME/DOMAIN pair:
//   subdomain  https://<prefix>.<DOMAIN>/<app>/<rest>   (Jacob's choice, 2026-10-27)
//   path       https://<DOMAIN>/<prefix>/<app>/<rest>
//   two-level  https://<app>.<prefix>.<DOMAIN>/<rest>
// Everything that is not a site (the apex, reserved hosts, existing zone records) passes through
// to the origin untouched, except `www`/`app`, which 301 to the apex.

const MANIFEST_TTL_S = 30;
const INDEX_TTL_S = 30;
const BLOB_TTL_S = 31536000;

// Hosts with their own DNS records that keep going to their origin (tunnels, services).
const PASSTHROUGH_HOSTS = new Set(["headless-engine", "sn120", "livekit", "turn", "relay", "voice-api", "hub-api"]);
// Legacy proxied names that showed a stale page from port 80: send them to the landing.
const REDIRECT_TO_APEX = new Set(["www", "app"]);

const RESERVED = new Set([
  "www", "api", "app", "relay", "admin", "mail", "m", "static", "cdn", "assets", "blob", "img",
  "media", "docs", "help", "support", "status", "dev", "staging", "test", "beta", "login", "auth",
  "oauth", "sso", "account", "accounts", "billing", "pay", "shop", "store", "blog", "news",
  "mx", "smtp", "imap", "pop", "pop3", "ftp", "sftp", "ns", "ns1", "ns2", "ns3", "ns4",
  "autodiscover", "autoconfig", "webmail", "email", "mta", "dmarc", "dkim", "spf", "acme",
  "livekit", "turn", "headless-engine", "sn120",
  "arbos", "sites", "site", "edge", "worker", "workers", "r2", "preview", "demo", "ops", "root",
  "localhost", "local", "internal", "proxy", "vpn", "git", "ci", "hub", "voice", "voice-api",
  "hub-api", "download", "downloads", "update", "updates", "release", "releases",
  "s", "g", "i", "privacy", "welcome", "u",
]);

const SUB_RE = /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/;
const APP_RE = /^[a-z0-9](?:[a-z0-9-]{0,38}[a-z0-9])?$/;
const SEGMENT_OK = /^[^\\\0-\x1f:*?"<>|]+$/;

export default {
  async fetch(request, env, ctx) {
    const url = new URL(request.url);
    const domain = (env.DOMAIN || "arbos.life").toLowerCase();
    const scheme = (env.SCHEME || "subdomain").toLowerCase();
    const host = url.hostname.toLowerCase();

    if (host === domain) return fetch(request);
    if (!host.endsWith("." + domain)) return fetch(request);
    const labels = host.slice(0, -(domain.length + 1));

    if (REDIRECT_TO_APEX.has(labels)) {
      return Response.redirect(`https://${domain}${url.pathname}${url.search}`, 301);
    }
    if (PASSTHROUGH_HOSTS.has(labels)) return fetch(request);

    const resolved = resolve(scheme, labels, url.pathname);
    if (resolved === "passthrough") return fetch(request);

    if (request.method !== "GET" && request.method !== "HEAD") {
      return text(405, "Method not allowed", { Allow: "GET, HEAD" });
    }
    if (env.SERVE === "0") return page(503, "Sites are paused", "Arbos sites are temporarily paused. Please try again later.", env);

    // A user landing: `https://<prefix>.<domain>/`
    if (resolved.kind === "user") {
      const moved = await movedTo(env, resolved.prefix);
      if (moved) return Response.redirect(moved.to_url || `https://${moved.to}.${domain}/`, 301);
      const index = await readJson(env, `sites/${resolved.prefix}/index.json`, INDEX_TTL_S);
      if (!index) return page(404, "No site here", `There is no published site at <code>${esc(host)}</code>.`, env);
      return userIndex(index, resolved.prefix, domain, env);
    }
    if (resolved.kind === "bad") {
      return page(404, "Not found", "That address is not a valid site path.", env);
    }

    const { prefix, app, rest, needsSlash } = resolved;
    if (needsSlash) {
      return Response.redirect(`${url.protocol}//${host}${url.pathname}/${url.search}`, 301);
    }
    const manifest = await readJson(env, `sites/${prefix}/${app}/manifest.json`, MANIFEST_TTL_S);
    if (!manifest) {
      const moved = await movedTo(env, prefix);
      if (moved) {
        const target = scheme === "subdomain"
          ? `https://${moved.to}.${domain}${url.pathname}${url.search}`
          : (moved.to_url || `https://${domain}/`);
        return Response.redirect(target, 301);
      }
      return page(404, "No site here", `Nothing is published at <code>${esc(host)}/${esc(app)}/</code>.`, env);
    }
    if (manifest.status !== "live") {
      return page(410, "This site was taken down",
        manifest.reason ? `Reason: ${esc(manifest.reason)}` : "It was removed by Arbos.", env,
        { "X-Robots-Tag": "noindex" });
    }

    let key = rest === "" ? "index.html" : rest.endsWith("/") ? rest + "index.html" : rest;
    key = decodeURIComponentSafe(key);
    if (!validKey(key)) return page(404, "Not found", "That path is not valid.", env);

    let entry = manifest.files[key];
    let status = 200;
    if (!entry && key === "robots.txt") {
      return text(200, "User-agent: *\nAllow: /\n", { "Content-Type": "text/plain; charset=utf-8", "Cache-Control": "public, max-age=3600" });
    }
    if (!entry && !key.includes(".")) {
      // `/about` → `about.html`, then `about/index.html` (static-host convention).
      entry = manifest.files[key + ".html"] || manifest.files[key + "/index.html"];
    }
    if (!entry) {
      entry = manifest.files["404.html"];
      status = 404;
      if (!entry) return page(404, "Not found", `<code>${esc(key)}</code> is not part of this site.`, env);
    }

    const etag = `"${entry.sha256}"`;
    if (request.headers.get("If-None-Match") === etag && status === 200) {
      return new Response(null, { status: 304, headers: { ETag: etag } });
    }
    const blob = await readBlob(env, ctx, entry.sha256);
    if (!blob) return page(502, "Missing file", "The site's file is not available right now. Try again in a minute.", env);

    const isHtml = (entry.content_type || "").startsWith("text/html");
    const headers = new Headers({
      "Content-Type": entry.content_type || "application/octet-stream",
      "Content-Length": String(entry.size),
      ETag: etag,
      "Cache-Control": isHtml ? "no-cache" : "public, max-age=300",
      "X-Content-Type-Options": "nosniff",
      "Referrer-Policy": "strict-origin-when-cross-origin",
      "X-Arbos-Site": `${prefix}/${app}@${manifest.version}`,
    });
    if (status === 404) headers.set("X-Robots-Tag", "noindex");
    return new Response(request.method === "HEAD" ? null : blob, { status, headers });
  },
};

/** The one routing function (keep in step with `arbos_core::sites::Route::parse_request`). */
function resolve(scheme, labels, pathname) {
  const segs = pathname.replace(/^\/+/, "").split("/");
  if (scheme === "subdomain") {
    if (labels.includes(".")) return "passthrough";
    if (RESERVED.has(labels) || labels.startsWith("_") || !SUB_RE.test(labels)) return "passthrough";
    if (segs.length === 1 && segs[0] === "") return { kind: "user", prefix: labels };
    const app = segs[0];
    if (!APP_RE.test(app)) return { kind: "bad" };
    if (segs.length === 1) return { kind: "site", prefix: labels, app, rest: "", needsSlash: true };
    return { kind: "site", prefix: labels, app, rest: segs.slice(1).join("/"), needsSlash: false };
  }
  if (scheme === "two-level") {
    const i = labels.indexOf(".");
    if (i < 0) return "passthrough";
    const app = labels.slice(0, i);
    const prefix = labels.slice(i + 1);
    if (prefix.includes(".") || RESERVED.has(prefix) || !SUB_RE.test(prefix) || !APP_RE.test(app)) return "passthrough";
    return { kind: "site", prefix, app, rest: segs.join("/").replace(/^\/+/, ""), needsSlash: false };
  }
  // path scheme: only on the apex, which this Worker does not front; everything else passes.
  return "passthrough";
}

function validKey(key) {
  if (key.length === 0 || key.length > 512) return false;
  const parts = key.split("/");
  if (parts.length > 8) return false;
  for (const p of parts) {
    if (p === "" || p === "." || p === ".." || p.startsWith(".") || !SEGMENT_OK.test(p)) return false;
  }
  return true;
}

function decodeURIComponentSafe(s) {
  try { return decodeURIComponent(s); } catch { return "\0"; }
}

async function readJson(env, key, ttl) {
  const cache = caches.default;
  const cacheKey = new Request(`https://sites-cache.arbos.internal/${key}`);
  let res = await cache.match(cacheKey);
  if (!res) {
    const obj = await env.SITES.get(key);
    if (!obj) {
      // Negative cache, shorter, so a fresh publish shows within the TTL.
      res = new Response("null", { headers: { "Content-Type": "application/json", "Cache-Control": `max-age=${Math.min(ttl, 10)}` } });
    } else {
      res = new Response(await obj.text(), { headers: { "Content-Type": "application/json", "Cache-Control": `max-age=${ttl}` } });
    }
    await cache.put(cacheKey, res.clone());
  }
  try {
    const v = await res.json();
    return v && typeof v === "object" ? v : null;
  } catch {
    return null;
  }
}

async function movedTo(env, prefix) {
  const r = await readJson(env, `sites/${prefix}/redirect.json`, INDEX_TTL_S);
  if (!r || !r.to) return null;
  if (r.until && Date.now() > r.until) return null;
  return r;
}

async function readBlob(env, ctx, sha) {
  const cache = caches.default;
  const cacheKey = new Request(`https://sites-cache.arbos.internal/blobs/${sha}`);
  const hit = await cache.match(cacheKey);
  if (hit) return hit.body;
  const obj = await env.SITES.get(`blobs/${sha}`);
  if (!obj) return null;
  const [a, b] = obj.body.tee();
  ctx.waitUntil(cache.put(cacheKey, new Response(b, { headers: { "Cache-Control": `public, max-age=${BLOB_TTL_S}, immutable` } })));
  return a;
}

function esc(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
}

function text(status, body, headers = {}) {
  return new Response(body, { status, headers: { "Content-Type": "text/plain; charset=utf-8", ...headers } });
}

function page(status, title, body, env, extra = {}) {
  const abuse = env.ABUSE_EMAIL ? `<p class="s">Something wrong with a site? <a href="mailto:${esc(env.ABUSE_EMAIL)}">Report it</a>.</p>` : "";
  const html = `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>${esc(title)} · Arbos</title><style>body{font:16px/1.5 system-ui,-apple-system,sans-serif;background:#0b0b0c;color:#eaeaea;margin:0;display:grid;place-items:center;min-height:100vh}main{max-width:32rem;padding:2rem}h1{font-size:1.5rem;margin:0 0 .5rem}code{background:#1a1a1c;padding:.1em .35em;border-radius:4px}a{color:#9bb0ff}.s{color:#9a9a9a;font-size:.9rem}</style></head><body><main><h1>${esc(title)}</h1><p>${body}</p>${abuse}<p class="s">Hosted by <a href="https://${esc(env.DOMAIN || "arbos.life")}/">Arbos</a>.</p></main></body></html>`;
  return new Response(html, { status, headers: { "Content-Type": "text/html; charset=utf-8", "Cache-Control": "no-store", "X-Content-Type-Options": "nosniff", ...extra } });
}

function userIndex(index, prefix, domain, env) {
  const sites = Array.isArray(index.sites) ? index.sites : [];
  const items = sites.map((s) => `<li><a href="${esc(s.url || `/${s.app}/`)}">${esc(s.app)}</a></li>`).join("");
  const body = sites.length
    ? `<ul>${items}</ul>`
    : `<p>${esc(prefix)} has no published sites right now.</p>`;
  const html = `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>${esc(prefix)} · Arbos sites</title><style>body{font:16px/1.5 system-ui,-apple-system,sans-serif;background:#0b0b0c;color:#eaeaea;margin:0;display:grid;place-items:center;min-height:100vh}main{max-width:32rem;padding:2rem}h1{font-size:1.5rem;margin:0 0 1rem}ul{padding-left:1.2rem}a{color:#9bb0ff}.s{color:#9a9a9a;font-size:.9rem}</style></head><body><main><h1>${esc(prefix)}.${esc(domain)}</h1>${body}<p class="s">Sites built by ${esc(prefix)}'s agent, hosted by <a href="https://${esc(domain)}/">Arbos</a>.</p></main></body></html>`;
  return new Response(html, { status: 200, headers: { "Content-Type": "text/html; charset=utf-8", "Cache-Control": "no-cache", "X-Content-Type-Options": "nosniff" } });
}

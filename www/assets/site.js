// Point the Mac download at the newest Arbos-*.dmg on GitHub. Without this
// (or if the API is unreachable) the link goes to the releases page.
(function () {
  var links = document.querySelectorAll("[data-dmg]");
  if (!links.length || !window.fetch) return;
  var api = "https://api.github.com/repos/unarbos/arbos/releases/latest";
  fetch(api, { headers: { Accept: "application/vnd.github+json" } })
    .then(function (r) { return r.ok ? r.json() : null; })
    .then(function (rel) {
      if (!rel || !rel.assets) return;
      var dmg = rel.assets.filter(function (a) {
        return /^Arbos-.*\.dmg$/i.test(a.name);
      })[0];
      if (!dmg) return;
      links.forEach(function (a) {
        a.href = dmg.browser_download_url;
        a.setAttribute("download", dmg.name);
      });
      var tag = document.querySelectorAll("[data-release-tag]");
      tag.forEach(function (el) { el.textContent = rel.tag_name; });
      var note = document.querySelectorAll("[data-release-note]");
      note.forEach(function (el) {
        el.textContent = " · " + dmg.name + " · " + Math.round(dmg.size / 1048576) + " MB";
        el.hidden = false;
      });
    })
    .catch(function () {});
})();

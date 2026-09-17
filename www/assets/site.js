// The button's href is /download/mac, a 302 to the newest published Mac build
// that the server refreshes itself. /download/mac.json is the same pick as
// data; use it to link the file directly and to name the build by the button.
(function () {
  var links = document.querySelectorAll("[data-mac-download]");
  if (!links.length || !window.fetch) return;
  fetch("/download/mac.json", { cache: "no-cache" })
    .then(function (r) { return r.ok ? r.json() : null; })
    .then(function (pick) {
      if (!pick || !pick.url) return;
      links.forEach(function (a) { a.href = pick.url; });
      var what = pick.channel === "dev"
        ? "dev build " + pick.version + "+" + pick.build
        : "Arbos " + pick.version;
      var size = pick.size ? Math.round(pick.size / 1048576) + " MB " : "";
      var kind = pick.format === "dmg" ? "disk image" : "zip";
      document.querySelectorAll("[data-release-note]").forEach(function (el) {
        el.textContent = what + ", " + size + kind;
      });
    })
    .catch(function () {});
})();

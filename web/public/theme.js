// Silicon UI themes follow :root[data-theme]; the site follows the system appearance. Loaded
// synchronously in <head>, so the first paint already has the right theme.
(function () {
  var preference = window.matchMedia("(prefers-color-scheme: dark)");
  var update = function () { document.documentElement.dataset.theme = preference.matches ? "dark" : "light"; };
  update();
  preference.addEventListener("change", update);
})();

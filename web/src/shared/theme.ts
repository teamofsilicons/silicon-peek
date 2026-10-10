const preference = window.matchMedia("(prefers-color-scheme: dark)");
const update = () => { document.documentElement.dataset.theme = preference.matches ? "dark" : "light"; };
update();
preference.addEventListener("change", update);

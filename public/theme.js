function setTheme(theme) {
  document.documentElement.style.colorScheme = theme;
  document.getElementById("theme-toggle").setAttribute("aria-label", `切换到${theme === "dark" ? "浅" : "深"}色外观`);
  document.querySelector('meta[name="theme-color"]').content = theme === "dark" ? "#111315" : "#f5f3ee";
}
let theme = "light";
try { if (localStorage.getItem("vitrine-theme") === "dark") theme = "dark"; } catch { /* Storage can be unavailable in private browsing. */ }
setTheme(theme);
document.getElementById("theme-toggle").addEventListener("click", () => {
  theme = theme === "dark" ? "light" : "dark";
  setTheme(theme);
  try { localStorage.setItem("vitrine-theme", theme); } catch { /* The current page still respects the choice. */ }
});

function syncSearchViewport() {
  const viewport = window.visualViewport;
  // The software keyboard can pan the visual viewport away from a fixed input.
  const offset = document.activeElement === document.getElementById("q") && viewport?.scale === 1 ? Math.max(0, viewport.offsetTop) : 0;
  document.querySelector(".topbar").style.setProperty("--viewport-offset", `${offset}px`);
}
document.getElementById("q").addEventListener("focus", syncSearchViewport);
document.getElementById("q").addEventListener("blur", syncSearchViewport);
window.visualViewport?.addEventListener("scroll", syncSearchViewport, { passive: true });
window.visualViewport?.addEventListener("resize", syncSearchViewport, { passive: true });

function updateBackToTop() {
  document.getElementById("back-to-top").hidden = window.scrollY < 400;
}
function backToTop() {
  document.querySelector("main").focus({ preventScroll: true });
  window.scrollTo({ top: 0, behavior: "auto" });
}
document.getElementById("back-to-top").addEventListener("click", backToTop);
window.addEventListener("scroll", updateBackToTop, { passive: true });
updateBackToTop();

// Loaded without defer, ahead of the stylesheets, so a saved or system dark theme never flashes light.
// theme.js reads the scheme set here and wires up the toggle once the page has parsed.
{
  let scheme = matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
  try {
    const saved = localStorage.getItem("vitrine-theme");
    if (saved === "dark" || saved === "light") scheme = saved;
  } catch { /* Storage can be unavailable in private browsing. */ }
  document.documentElement.style.colorScheme = scheme;
}

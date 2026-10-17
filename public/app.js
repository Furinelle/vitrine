const $ = (id) => document.getElementById(id);
const names = { pixiv: "Pixiv", x: "X / Twitter", douyin: "抖音" };
// 中文浏览器显示标签的中文名，其他语言显示英文名，没有英文名时仍用中文名。
const zh = /^zh\b/i.test(navigator.languages?.[0] || navigator.language || "");
const tagInfo = new Map();
const tagName = (tag) => (zh ? tag : tagInfo.get(tag)?.en || tag);
const tagKinds = zh
  ? { work: "作品", character: "角色", cp: "CP", skin: "皮肤", lore: "设定", theme: "题材", medium: "媒介", meta: "其他", unsorted: "未分类" }
  : { work: "Works", character: "Characters", cp: "Pairings", skin: "Skins", lore: "Lore", theme: "Themes", medium: "Media", meta: "Meta", unsorted: "Other" };
// tag 可以是逗号分隔的多个标签（命中任一即可）；界面上只写第一个和总数。
function tagLabel(tag) {
  const tags = tag.split(",").filter(Boolean);
  return tags.length > 1 ? `${tagName(tags[0])} 等 ${tags.length} 个标签` : tagName(tag);
}
function locationFilters() {
  const query = new URLSearchParams(location.search);
  return { source: query.get("source") || "", tag: query.get("tag") || "", q: query.get("q") || "",
    month: /^\d{4}-\d{2}$/.test(query.get("month") || "") ? query.get("month") : "", p: Math.max(1, parseInt(query.get("p"), 10) || 1) };
}
// The collection is shown as a hall of pages: one collection month at a time (Beijing time), fifteen
// works to a page set as one large, four medium and ten small pictures.
const PAGE_SIZE = 15;
const state = {
  ...locationFilters(), works: [], months: [], loading: false,
  request: null, sources: [], tags: [], tagError: "", viewerRequest: null, viewing: null, images: [], imageIndex: 0,
};
function node(tag, className, text) {
  const element = document.createElement(tag);
  if (className) element.className = className;
  if (text !== undefined) element.textContent = text;
  return element;
}
function button(className, text, action) {
  const element = node("button", className, text);
  element.type = "button";
  element.addEventListener("click", action);
  return element;
}
function safeLink(value) {
  try {
    const url = new URL(value);
    return ["https:", "http:"].includes(url.protocol) && !url.username && !url.password ? url.href : "";
  } catch { return ""; }
}
function mediaUrl(key) {
  return "/media/" + String(key).split("/").map(encodeURIComponent).join("/");
}
function coverUrl(value) {
  if (!value) return "";
  try {
    const url = new URL(value, location.origin);
    return url.origin === location.origin && url.pathname.startsWith("/media/") ? url.href : "";
  } catch { return ""; }
}
function thumbUrl(value, width = 640) {
  const original = coverUrl(value);
  if (!original || ![160, 640, 1280].includes(width)) return "";
  const url = new URL(original);
  url.search = new URLSearchParams({ width });
  return url.href;
}
async function getJson(url, signal) {
  const response = await fetch(url, { signal, headers: { Accept: "application/json" } });
  if (response.status === 429) throw new Error("访问较频繁，请稍等一分钟再试");
  if (response.status === 404) throw new Error("这件作品已下线或不存在");
  if (!response.ok) throw new Error(`请求失败（${response.status}）`);
  const data = await response.json();
  if (!data.ok) throw new Error("暂时无法读取图库");
  return data;
}
function galleryUrl(workId = "", page = 0) {
  const query = new URLSearchParams();
  for (const field of ["source", "tag", "q", "month"]) if (state[field]) query.set(field, state[field]);
  if (state.p > 1) query.set("p", String(state.p));
  if (workId) query.set("work", workId);
  if (workId && page > 0) query.set("page", String(page + 1));
  return "/" + (query.size ? `?${query}` : "");
}
function syncFilters(preserveDraft = false) {
  $("source").value = state.source;
  if (!preserveDraft) { $("tag").value = state.tag; $("q").value = state.q; }
  document.querySelectorAll(".source-link").forEach((item) => {
    const active = item.dataset.source === state.source;
    item.classList.toggle("active", active);
    if (active) item.setAttribute("aria-current", "page");
    else item.removeAttribute("aria-current");
  });
  document.querySelectorAll("#popular-tags .tag-chip").forEach((item) => item.classList.toggle("active", item.dataset.tag === state.tag));
  placeTabRule();
  const title = state.tag ? `# ${tagLabel(state.tag)}` : state.q ? "搜索结果" : state.source ? names[state.source] || state.source : "全部作品";
  $("collection-title").textContent = title;
  if (!$("lightbox").open) document.title = `${state.source || state.tag || state.q ? title + " · " : ""}Vitrine · 图库`;
  $("reset").hidden = !state.source && !state.tag && !state.q;
  $("active-filters").replaceChildren();
  for (const field of ["tag", "q"]) {
    if (!state[field]) continue;
    const value = field === "tag" ? tagLabel(state.tag) : state.q;
    const chip = button("active-filter", `${field === "tag" ? "# " : "搜索："}${value} ×`, () => applyFilters({ [field]: "" }));
    chip.setAttribute("aria-label", `移除${field === "tag" ? "标签" : "搜索"}筛选：${value}`);
    $("active-filters").append(chip);
  }
}
// One rule under the source tabs slides to the chosen tab instead of each tab drawing its own.
function placeTabRule() {
  const rule = document.querySelector(".tab-rule"), active = document.querySelector("#sources .source-link.active");
  if (!rule || !active) return;
  rule.style.width = `${active.offsetWidth}px`;
  rule.style.translate = `${active.offsetLeft}px 0`;
}
function updateCount() {
  // The month counts cover any filter; before they arrive, the source totals cover the unfiltered collection.
  const sources = state.source ? state.sources.find((s) => s.source === state.source)?.cnt : state.sources.length ? state.sources.reduce((sum, s) => sum + s.cnt, 0) : null;
  const total = state.months.length ? state.months.reduce((sum, month) => sum + month.cnt, 0) : !state.tag && !state.q ? sources : null;
  $("collection-count").textContent = total != null ? `${total.toLocaleString("zh-CN")} 件收藏` : "";
}
function applyFilters(changes) {
  // New filters start from their newest month.
  Object.assign(state, { month: "", p: 1 }, changes);
  if ($("lightbox").open) {
    state.viewerRequest?.abort(); state.viewerRequest = null;
    dismissLightbox();
  }
  history.pushState(null, "", galleryUrl());
  syncFilters();
  loadHall();
  window.scrollTo({ top: 0, behavior: "instant" });
}
async function loadFilters() {
  await Promise.allSettled([getJson("/api/sources").then((data) => {
    state.sources = data.sources || [];
    $("sources").replaceChildren();
    $("source").replaceChildren(new Option("全部来源", ""));
    const all = [{ source: "", cnt: state.sources.reduce((sum, s) => sum + s.cnt, 0) }, ...state.sources];
    for (const source of all) {
      const label = names[source.source] || source.source || "全部作品";
      const item = button("source-link", undefined, () => applyFilters({ source: source.source }));
      item.dataset.source = source.source;
      item.append(node("span", "", label), node("span", "source-count", source.cnt.toLocaleString("zh-CN")));
      $("sources").append(item);
      if (source.source) $("source").append(new Option(label, source.source));
    }
    $("sources").append(node("span", "tab-rule"));
    if (state.source && !state.sources.some((s) => s.source === state.source)) $("source").append(new Option(state.source, state.source));
    syncFilters(true);
    updateCount();
  }), loadTags()]);
}
function loadTags() {
  // The whole tag list (a few hundred rows, cached by the Worker) backs display names and the grouped index.
  return getJson("/api/tags?limit=1000").then((data) => {
    state.tags = data.tags || [];
    state.tagError = "";
    tagInfo.clear();
    for (const tag of state.tags) tagInfo.set(tag.name, tag);
    $("taglist").replaceChildren(...state.tags.map((tag) => new Option(`${tagName(tag.name)} · ${tag.cnt} 件作品`, tag.name)));
    $("popular-tags").replaceChildren(...state.tags.slice(0, 8).map((tag) => tagButton(tag.name)));
    syncFilters(true);
    if (!$("tag-browser").hidden) renderTagBrowser();
  }, (error) => {
    state.tagError = error.message;
    if (!$("tag-browser").hidden) renderTagBrowser();
  });
}
// The work a character, skin or setting belongs to, following parents up to a work.
function tagWork(tag) {
  for (let info = tagInfo.get(tag.parent), hops = 0; info && hops < 4; info = tagInfo.get(info.parent), hops++) {
    if (info.kind === "work") return info.name;
  }
  return "";
}
function renderTagBrowser() {
  const needle = $("tag-search").value.trim().replace(/^#+/, "").toLowerCase();
  const groups = new Map(Object.keys(tagKinds).map((kind) => [kind, []]));
  for (const tag of state.tags) {
    if (needle && ![tag.name, tag.ja, tag.en].some((name) => name?.toLowerCase().includes(needle))) continue;
    groups.get(tag.kind in tagKinds ? tag.kind : "unsorted").push(tag);
  }
  // The most used tags read larger: the top twentieth of the whole list, then the next fifth.
  const counts = state.tags.map((tag) => tag.cnt).sort((a, b) => b - a);
  const tier1 = counts[Math.floor(counts.length * .05)] ?? Infinity, tier2 = counts[Math.floor(counts.length * .25)] ?? Infinity;
  const chips = (tags) => {
    const list = node("div", "popular-tags");
    list.append(...tags.map((tag) => {
      const chip = tagButton(tag.name);
      chip.classList.add(tag.cnt >= tier1 ? "tier-1" : tag.cnt >= tier2 ? "tier-2" : "tier-3");
      chip.classList.toggle("active", tag.name === state.tag);
      const count = node("span", "tag-count", tag.cnt.toLocaleString("zh-CN"));
      count.setAttribute("aria-label", `${tag.cnt} 件作品`);
      chip.append(count);
      return chip;
    }));
    return list;
  };
  const shown = [...groups].filter(([, tags]) => tags.length);
  $("tag-kinds").replaceChildren(...shown.map(([kind, tags]) => {
    const jump = button(`tag-kind kind-${kind}`, undefined, () => {
      const box = $("all-tags"), target = box.querySelector(`[data-kind="${kind}"]`);
      box.scrollTo({ top: target.offsetTop - box.offsetTop, behavior: matchMedia("(prefers-reduced-motion: reduce)").matches ? "auto" : "smooth" });
    });
    jump.append(node("span", "", tagKinds[kind]), node("span", "tag-count", String(tags.length)));
    return jump;
  }));
  $("all-tags").replaceChildren(...shown.map(([kind, tags], index) => {
    const group = node("section", `tag-group kind-${kind}`);
    group.dataset.kind = kind;
    group.style.setProperty("--i", String(index));
    const title = node("h3", "tag-group-title");
    title.append(node("span", "", tagKinds[kind]), node("span", "tag-count", String(tags.length)));
    group.append(title);
    if (kind === "character" || kind === "lore" || kind === "skin") {
      // Split by work, in the order each work first appears (tags arrive most used first).
      const byWork = new Map();
      for (const tag of tags) {
        const work = tagWork(tag);
        if (!byWork.has(work)) byWork.set(work, []);
        byWork.get(work).push(tag);
      }
      const noWork = byWork.get("");
      byWork.delete("");
      if (noWork) byWork.set("", noWork);
      for (const [work, list] of byWork) {
        if (byWork.size > 1) group.append(node("h4", "tag-work-title", work ? tagName(work) : tagKinds.unsorted));
        group.append(chips(list));
      }
    } else group.append(chips(tags));
    return group;
  }));
  $("tag-empty").hidden = !!state.tagError || !state.tags.length || $("all-tags").children.length > 0;
  $("tag-status").textContent = state.tagError ? `${state.tagError}。` : state.tags.length ? "" : "加载标签…";
  $("tag-retry").hidden = !state.tagError;
}
function tagButton(tag) {
  const chip = button(`tag-chip kind-${tagInfo.get(tag)?.kind || "unsorted"}`, tagName(tag), async () => {
    const fromDialog = $("lightbox").open;
    if (fromDialog) await dismissLightbox();
    applyFilters({ tag });
    if (fromDialog) $("collection").focus();
  });
  chip.dataset.tag = tag;
  return chip;
}
// Native lazy-loading can fetch several screens ahead; keep gallery traffic near the viewport.
function coverRatio(width, height) {
  return width > 0 && height > 0 ? Math.min(1.8, Math.max(.5, width / height)) : 0;
}
// The artwork's own colour, read from an 8×8 copy: colourful pixels outweigh white paper and line art,
// and the result is lifted to a saturation and lightness that still shows as a glow on either theme.
// The artwork's own colour, read from an 8×8 copy: colourful pixels outweigh white paper and line art,
// and the result is lifted to a saturation and lightness that still shows as a glow on either theme.
let swatchContext;
function artColor(image) {
  try {
    if (!swatchContext) {
      const swatch = document.createElement("canvas");
      swatch.width = swatch.height = 8;
      swatchContext = swatch.getContext("2d", { willReadFrequently: true });
    }
    swatchContext.clearRect(0, 0, 8, 8);
    swatchContext.drawImage(image, 0, 0, 8, 8);
    const data = swatchContext.getImageData(0, 0, 8, 8).data;
    let r = 0, g = 0, b = 0, total = 0;
    for (let i = 0; i < data.length; i += 4) {
      const weight = 1 + (Math.max(data[i], data[i + 1], data[i + 2]) - Math.min(data[i], data[i + 1], data[i + 2])) / 24;
      r += data[i] * weight; g += data[i + 1] * weight; b += data[i + 2] * weight; total += weight;
    }
    [r, g, b] = [r / total / 255, g / total / 255, b / total / 255];
    const max = Math.max(r, g, b), min = Math.min(r, g, b), d = max - min;
    let hue = 0;
    if (d) hue = max === r ? ((g - b) / d) % 6 : max === g ? (b - r) / d + 2 : (r - g) / d + 4;
    const light = (max + min) / 2, sat = d ? d / (1 - Math.abs(2 * light - 1)) : 0;
    return `hsl(${Math.round(hue * 60 + 360) % 360} ${Math.round(Math.min(Math.max(sat, .45), .8) * 100)}% ${Math.round(Math.min(Math.max(light, .45), .62) * 100)}%)`;
  } catch {
    return "";
  }
}
const monthName = new Intl.DateTimeFormat(zh ? "zh-CN" : "en", { year: "numeric", month: "long", timeZone: "UTC" });
const monthLabel = (month) => monthName.format(new Date(`${month}-01T00:00:00Z`));
const pagesIn = (month) => Math.max(1, Math.ceil((state.months.find((m) => m.month === month)?.cnt || 0) / PAGE_SIZE));
function filterQuery(extra = {}) {
  const query = new URLSearchParams(extra);
  for (const field of ["source", "tag", "q"]) if (state[field]) query.set(field, state[field]);
  return query;
}
function hallItem(work, i) {
  const size = i === 0 ? "lead" : i < 5 ? "medium" : "small";
  const title = work.title || work.source_id;
  const card = node("article", `hall-item ${size}`);
  card.dataset.workId = work.id;
  card.style.setProperty("--i", String(i));
  middleObserver?.observe(card);
  const art = node("a", "art-button");
  art.href = galleryUrl(work.id);
  art.setAttribute("aria-label", `查看 ${title}，${work.page_count} 张图片`);
  const image = node("img");
  image.alt = "";
  image.decoding = "async";
  image.loading = i < 5 ? "eager" : "lazy";
  if (i === 0) image.fetchPriority = "high";
  image.onload = () => {
    art.classList.add("image-loaded");
    const color = artColor(image);
    if (color) {
      card.style.setProperty("--art", color);
      if (stage.cards.has(card)) stageColor();
    }
  };
  image.onerror = () => art.classList.add("image-broken");
  const original = coverUrl(work.cover_url);
  if (original) image.src = thumbUrl(original, size === "lead" ? 1280 : 640);
  else art.classList.add("image-broken");
  art.append(image, node("span", "image-fallback", "预览暂不可用 · 查看详情"));
  if (work.page_count > 1) art.append(node("span", "image-badge", `${work.page_count} 张`));
  const caption = node("div", "hall-caption");
  const meta = node("p", "card-meta");
  meta.append(node("span", "card-author", work.author_name || "未署名"), node("span", "card-source", names[work.source] || work.source));
  caption.append(node("h2", "hall-title", title), meta);
  art.append(caption);
  art.addEventListener("click", (event) => {
    if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
    event.preventDefault();
    openLightbox(work, card);
  });
  card.append(art);
  return card;
}
function renderLoadingHall() {
  for (const card of $("grid").children[0]?.children || []) middleObserver?.unobserve(card);
  stage.cards.clear();
  const spread = node("div", "spread loading");
  spread.setAttribute("aria-hidden", "true");
  for (let i = 0; i < PAGE_SIZE; i++) spread.append(node("div", `hall-item ${i === 0 ? "lead" : i < 5 ? "medium" : "small"} skeleton`));
  $("grid").replaceChildren(spread);
}
// The outgoing page slides away in the turning direction before the next one is set in.
async function renderHall(works, direction) {
  const old = $("grid").children[0];
  if (old && direction && !calm.matches && old.animate) {
    const from = { opacity: old.style.opacity || 1, translate: old.style.translate || "0 0" };
    const dragged = parseFloat(old.style.translate) || 0;
    const leaving = old.animate([from, { opacity: 0, translate: `${dragged - direction * 48}px 0` }],
      { duration: 240, easing: "cubic-bezier(.4, 0, 1, 1)", fill: "forwards" }).finished.catch(() => {});
    // A throttled tab may never finish the animation; the turn must not wait on it.
    await Promise.race([leaving, new Promise((resolve) => setTimeout(resolve, 300))]);
  }
  for (const card of old?.children || []) middleObserver?.unobserve(card);
  stage.cards.clear();
  const spread = node("div", "spread");
  spread.style.setProperty("--from", `${direction * 48}px`);
  works.forEach((work, i) => spread.append(hallItem(work, i)));
  $("grid").replaceChildren(spread);
}
const pageCache = new Map();
function fetchPage(month, p) {
  const query = filterQuery({ month, limit: PAGE_SIZE, offset: (p - 1) * PAGE_SIZE });
  const key = query.toString();
  if (!pageCache.has(key)) pageCache.set(key, getJson(`/api/works?${query}`).catch((error) => { pageCache.delete(key); throw error; }));
  return pageCache.get(key);
}
// The page one step away, crossing into the neighbouring month at either end; null past the ends.
function neighbour(step) {
  const index = state.months.findIndex((m) => m.month === state.month);
  const p = state.p + step;
  if (p >= 1 && p <= pagesIn(state.month)) return { month: state.month, p };
  const month = state.months[index + step]?.month;
  return month ? { month, p: step > 0 ? 1 : pagesIn(month) } : null;
}
let hallTicket = 0;
async function showPage(direction = 0) {
  const ticket = ++hallTicket;
  state.loading = true;
  $("grid").setAttribute("aria-busy", "true");
  $("status").textContent = "";
  $("status").classList.remove("error");
  $("retry").hidden = true;
  syncPager();
  try {
    const data = await fetchPage(state.month, state.p);
    if (ticket !== hallTicket) return;
    state.works = data.works || [];
    await renderHall(state.works, direction);
    if (ticket !== hallTicket) return;
    $("empty").hidden = state.works.length > 0;
    renderFeatured();
    // Ready the next and previous pages so a turn shows at once; the next page's large and medium covers too.
    for (const step of [1, -1]) {
      const next = neighbour(step);
      if (next) fetchPage(next.month, next.p).then((page) => {
        if (step > 0) page.works?.slice(0, 5).forEach((work, i) => { if (coverUrl(work.cover_url)) new Image().src = thumbUrl(coverUrl(work.cover_url), i ? 640 : 1280); });
      }, () => {});
    }
  } catch (error) {
    if (ticket !== hallTicket) return;
    $("status").textContent = `${error.message}。`;
    $("status").classList.add("error");
    $("grid").replaceChildren();
    $("retry").hidden = false;
  } finally {
    if (ticket === hallTicket) {
      state.loading = false;
      $("grid").setAttribute("aria-busy", "false");
    }
  }
}
// Months for the current filters, then the page the address asks for (newest month by default).
async function loadHall() {
  state.request?.abort();
  const controller = new AbortController();
  state.request = controller;
  hallTicket++;
  pageCache.clear();
  renderLoadingHall();
  $("empty").hidden = true;
  $("retry").hidden = true;
  $("status").textContent = "正在加载作品…";
  $("status").classList.remove("error");
  try {
    const data = await getJson(`/api/months?${filterQuery()}`, controller.signal);
    if (state.request !== controller) return;
    state.months = data.months || [];
    updateCount();
    renderMonths();
    if (!state.months.length) {
      $("grid").replaceChildren();
      $("status").textContent = "";
      $("empty").hidden = false;
      syncPager();
      return;
    }
    if (!state.months.some((m) => m.month === state.month)) Object.assign(state, { month: state.months[0].month, p: 1 });
    state.p = Math.min(state.p, pagesIn(state.month));
    history.replaceState(history.state, "", galleryUrl(new URLSearchParams(location.search).get("work") || "", Math.max(0, (parseInt(new URLSearchParams(location.search).get("page"), 10) || 1) - 1)));
    $("status").textContent = "";
    await showPage(0);
  } catch (error) {
    if (controller.signal.aborted || state.request !== controller) return;
    $("status").textContent = `${error.message}。`;
    $("status").classList.add("error");
    $("grid").replaceChildren();
    $("retry").hidden = false;
  }
}
function goToPage(month, p, direction) {
  if (month === state.month && p === state.p) return;
  Object.assign(state, { month, p });
  history.pushState(null, "", galleryUrl());
  renderMonths();
  showPage(direction);
  const top = $("months").getBoundingClientRect().top;
  if (top < 0) $("months").scrollIntoView({ behavior: calm.matches ? "auto" : "smooth", block: "start" });
}
function turn(step) {
  const next = neighbour(step);
  if (next && !state.loading) goToPage(next.month, next.p, step);
}
function renderMonths() {
  $("months").replaceChildren(...state.months.map((m) => {
    const tab = button(`month-tab${m.month === state.month ? " active" : ""}`, undefined, () => goToPage(m.month, 1, m.month < state.month ? 1 : -1));
    if (m.month === state.month) tab.setAttribute("aria-current", "page");
    tab.append(node("span", "", monthLabel(m.month)), node("span", "tag-count", m.cnt.toLocaleString("zh-CN")));
    return tab;
  }));
}
// Page numbers: the first, the last and two either side of the current one, with gaps marked.
function syncPager() {
  const pages = pagesIn(state.month), shown = new Set([1, pages]);
  for (let p = state.p - 2; p <= state.p + 2; p++) if (p >= 1 && p <= pages) shown.add(p);
  const list = [];
  let last = 0;
  for (const p of [...shown].sort((a, b) => a - b)) {
    if (p - last > 1) list.push(node("li", "page-gap", "…"));
    const item = node("li");
    const jump = button(`page-number${p === state.p ? " active" : ""}`, String(p), () => goToPage(state.month, p, p > state.p ? 1 : -1));
    if (p === state.p) jump.setAttribute("aria-current", "page");
    jump.setAttribute("aria-label", `第 ${p} 页`);
    item.append(jump);
    list.push(item);
    last = p;
  }
  $("page-list").replaceChildren(...list);
  const before = neighbour(-1), after = neighbour(1);
  $("prev-page").disabled = !before;
  $("next-page").disabled = !after;
  // Crossing into another month names it, so the turn is not a surprise.
  $("prev-page").lastChild.textContent = before && before.month !== state.month ? monthLabel(before.month) : "上一页";
  $("next-page").firstChild.textContent = after && after.month !== state.month ? monthLabel(after.month) : "下一页";
  $("pager").hidden = !state.months.length;
}
function showImage(index, updateUrl = true, retry = false) {
  if (!state.images.length) return;
  const previous = state.imageIndex;
  state.imageIndex = Math.max(0, Math.min(Number.isFinite(index) ? index : 0, state.images.length - 1));
  const direction = Math.sign(state.imageIndex - previous);
  const image = state.images[state.imageIndex];
  const original = coverUrl(image.r2_key ? mediaUrl(image.r2_key) : image.cover_url);
  $("lb-original").hidden = !original;
  if (original) $("lb-original").href = original;
  else $("lb-original").removeAttribute("href");
  $("lb-counter").textContent = `${state.imageIndex + 1} / ${state.images.length}`;
  $("lb-prev").disabled = state.imageIndex === 0;
  $("lb-next").disabled = state.imageIndex === state.images.length - 1;
  $("lb-prev").hidden = $("lb-next").hidden = state.images.length < 2;
  $("lb-thumbs").querySelectorAll("button").forEach((thumb, i) => {
    thumb.classList.toggle("active", i === state.imageIndex);
    thumb.setAttribute("aria-pressed", String(i === state.imageIndex));
  });
  $("lb-img").alt = `${state.viewing?.title || state.viewing?.source_id || "作品"} · 第 ${state.imageIndex + 1} 张`;
  const width = ($("lb-stage").clientWidth - 72) * Math.min(window.devicePixelRatio || 1, 2) <= 640 ? 640 : 1280;
  const url = original ? thumbUrl(original, width) : "";
  if (retry || state.imageKey !== url) {
    state.imageKey = url;
    $("lb-image-error").hidden = true;
    $("lb-img").hidden = false;
    const ticket = {};
    state.imageRequest = ticket;
    const full = $("lb-img");
    const preview = $("lb-preview");
    full.classList.remove("ready");
    full.removeAttribute("src");
    preview.hidden = true;
    preview.onload = () => { if (state.imageRequest === ticket) preview.hidden = false; };
    preview.onerror = () => { if (state.imageRequest === ticket) preview.hidden = true; };
    const known = state.imageIndex === 0 ? state.lastCard?.querySelector(".art-button img")
      : $("lb-thumbs").querySelectorAll("img")[state.imageIndex];
    const small = known?.complete && known.naturalWidth ? known.currentSrc || known.src : original ? thumbUrl(original, 640) : "";
    if (small) {
      preview.src = small;
      preview.hidden = !(preview.complete && preview.naturalWidth);
    } else preview.removeAttribute("src");
    $("lb-image-loading").hidden = !url;
    full.onload = async () => {
      try { await full.decode(); } catch {
        if (state.imageRequest === ticket) full.onerror();
        return;
      }
      if (state.imageRequest !== ticket) return;
      full.classList.add("ready");
      $("lb-image-loading").hidden = true;
      // The new picture enters from the side the reader is moving toward.
      if (direction && !matchMedia("(prefers-reduced-motion: reduce)").matches) full.animate([
        { opacity: 0, transform: `translateX(${direction * 32}px)` }, { opacity: 1, transform: "none" },
      ], { duration: 420, easing: "cubic-bezier(.22, 1, .36, 1)" });
    };
    full.onerror = () => {
      if (state.imageRequest !== ticket) return;
      full.hidden = true;
      $("lb-image-loading").hidden = true;
      $("lb-image-error").hidden = false;
    };
    if (url) full.src = url;
    else full.onerror();
  }
  for (const near of [state.images[state.imageIndex + 1], state.images[state.imageIndex - 1]]) {
    const nearUrl = near?.r2_key ? thumbUrl(mediaUrl(near.r2_key), width) : "";
    if (nearUrl) new Image().src = nearUrl;
  }
  if (updateUrl && state.viewing) history.replaceState(history.state, "", galleryUrl(state.viewing.id, state.imageIndex));
}
function showWorkInfo(work) {
  $("lb-title").textContent = work.title || work.source_id || "正在加载作品…";
  document.title = `${$("lb-title").textContent} · Vitrine`;
  $("lb-source").textContent = names[work.source] || work.source || "";
  const author = safeLink(work.author_url);
  $("lb-author").replaceChildren();
  if (author) {
    const link = node("a", "", work.author_name || "查看创作者");
    link.href = author; link.target = "_blank"; link.rel = "noopener noreferrer";
    $("lb-author").append(link);
  } else $("lb-author").textContent = work.author_name || "未署名";
  const date = new Date(work.created_at);
  $("lb-info").textContent = `${work.page_count || state.images.length} 张图片${Number.isNaN(date.valueOf()) ? "" : ` · 收藏于 ${date.toLocaleDateString("zh-CN")}`}`;
  $("lb-tags").replaceChildren(...(work.tags || []).map(tagButton));
  const source = safeLink(work.source_url);
  $("lb-link").hidden = !source;
  if (source) $("lb-link").href = source;
  else $("lb-link").removeAttribute("href");
}
function animateLightbox(closing = false) {
  const dialog = $("lightbox");
  const reduced = matchMedia("(prefers-reduced-motion: reduce)").matches;
  const from = closing && !reduced ? getComputedStyle(dialog) : null;
  const current = from ? { opacity: from.opacity, transform: from.transform } : null;
  state.viewerMotion?.cancel();
  dialog.classList.toggle("is-closing", closing);
  state.viewerMotion = null;
  if (reduced || !dialog.animate) return null;
  const rest = { opacity: 1, transform: "translateY(0) scale(1)" };
  const small = { opacity: 0, transform: lightboxOrigin(dialog) };
  // Fade in over the first third so the panel is seen growing out of the card.
  const motion = dialog.animate(closing ? [current, small] : [small, { opacity: 1, offset: .35 }, rest], {
    duration: closing ? 320 : 520, easing: closing ? "cubic-bezier(.4, 0, .6, 1)" : "cubic-bezier(.22, 1, .36, 1)", fill: "both",
  });
  state.viewerMotion = motion;
  if (!closing) motion.finished.then(() => {
    if (state.viewerMotion === motion) { motion.cancel(); state.viewerMotion = null; }
  }, () => {});
  return motion;
}
// Grow the viewer out of (and back into) the card it was opened from.
function lightboxOrigin(dialog) {
  const card = state.lastCard?.isConnected ? state.lastCard.getBoundingClientRect() : null;
  const frame = dialog.getBoundingClientRect();
  if (!card?.width || !frame.width) { dialog.style.transformOrigin = "50% 60%"; return "translateY(18px) scale(.95)"; }
  const at = (point, start, size) => Math.max(0, Math.min(100, (point - start) / size * 100));
  dialog.style.transformOrigin = `${at(card.x + card.width / 2, frame.x, frame.width)}% ${at(card.y + card.height / 2, frame.y, frame.height)}%`;
  return `scale(${Math.min(.85, Math.max(.25, card.width / frame.width))})`;
}
async function dismissLightbox() {
  const dialog = $("lightbox");
  state.viewerRequest?.abort(); state.viewerRequest = null;
  state.imageRequest = null;
  if (!dialog.open || dialog.classList.contains("is-closing")) return;
  const motion = animateLightbox(true);
  if (motion) { try { await motion.finished; } catch { return; } }
  if (state.viewerMotion !== motion || !dialog.classList.contains("is-closing")) return;
  dialog.close();
  motion?.cancel();
  state.viewerMotion = null;
  dialog.classList.remove("is-closing");
}

async function openLightbox(work, cardEl, { push = true, page = 0 } = {}) {
  state.viewerRequest?.abort();
  const controller = new AbortController();
  state.viewerRequest = controller;
  state.viewing = work;
  state.images = work.cover_url ? [{ cover_url: work.cover_url }] : [];
  state.imageIndex = 0;
  state.lastCard = cardEl || null;
  $("lb-thumbs").replaceChildren();
  $("lb-status").textContent = "正在加载完整作品…";
  $("lb-retry").hidden = true;
  $("share-fallback").hidden = true;
  $("lb-share").textContent = "复制作品链接";
  state.imageRequest = null; state.imageKey = null;
  $("lb-img").removeAttribute("src");
  $("lb-img").classList.remove("ready");
  $("lb-img").hidden = true;
  $("lb-preview").removeAttribute("src");
  $("lb-preview").hidden = true;
  $("lb-image-loading").hidden = true;
  $("lb-image-error").hidden = true;
  $("lb-prev").hidden = $("lb-next").hidden = true;
  $("lb-original").hidden = true;
  $("lb-counter").textContent = "正在加载";
  showWorkInfo(work);
  if (push) history.pushState({ galleryViewer: true }, "", galleryUrl(work.id, page));
  const dialog = $("lightbox");
  const entering = !dialog.open || dialog.classList.contains("is-closing");
  if (!dialog.open) { dialog.showModal(); dialog.scrollTop = 0; }
  if (entering) animateLightbox();
  if (state.images.length && page === 0) showImage(0, false);
  try {
    const data = await getJson(`/api/works/${encodeURIComponent(work.id)}`, controller.signal);
    if (state.viewerRequest !== controller || !$("lightbox").open) return;
    if (!data.images?.length || data.work?.id !== work.id) throw new Error("作品暂时不可用");
    state.viewing = data.work;
    state.images = data.images;
    showWorkInfo(data.work);
    $("lb-status").textContent = "";
    if (state.images.length > 1) $("lb-thumbs").replaceChildren(...state.images.map((image, i) => {
      const thumb = button("thumb", undefined, () => showImage(i));
      thumb.setAttribute("aria-label", `查看第 ${i + 1} 张图片`);
      const img = node("img");
      img.src = thumbUrl(mediaUrl(image.r2_key), 160);
      img.alt = ""; img.loading = "lazy"; img.decoding = "async";
      thumb.append(img);
      return thumb;
    }));
    showImage(page);
  } catch (error) {
    if (controller.signal.aborted || state.viewerRequest !== controller || !$("lightbox").open) return;
    $("lb-status").textContent = `${error.message}。`;
    $("lb-retry").hidden = false;
    if (state.images.length) $("lb-counter").textContent = "封面预览";
  }
}
function closeLightbox() {
  if (!$("lightbox").open || state.closing) return;
  state.viewerRequest?.abort();
  state.viewerRequest = null;
  if (history.state?.galleryViewer) { state.closing = true; history.back(); }
  else {
    history.replaceState(null, "", galleryUrl());
    dismissLightbox();
  }
}
function syncRoute() {
  state.closing = false;
  const filters = locationFilters();
  const changed = ["source", "tag", "q"].some((key) => filters[key] !== state[key]);
  const turned = filters.month !== state.month || filters.p !== state.p;
  const direction = filters.month === state.month ? Math.sign(filters.p - state.p) : filters.month < state.month ? 1 : -1;
  Object.assign(state, filters);
  if (changed) { syncFilters(); loadHall(); }
  else if (turned && state.months.length) { renderMonths(); showPage(direction); }
  const query = new URLSearchParams(location.search);
  const id = query.get("work");
  if (id) {
    const page = Math.max(0, (parseInt(query.get("page"), 10) || 1) - 1);
    if (state.viewing?.id === id && $("lightbox").open && !$("lightbox").classList.contains("is-closing")) showImage(page, false);
    else openLightbox(state.works.find((work) => work.id === id) || { id }, null, { push: false, page });
  } else if ($("lightbox").open) dismissLightbox();
  else syncFilters();
}

$("search-form").addEventListener("submit", (event) => { event.preventDefault(); applyFilters({ q: $("q").value.trim() }); });
$("filter-form").addEventListener("submit", (event) => { event.preventDefault(); applyFilters({ source: $("source").value, tag: $("tag").value.trim(), q: $("q").value.trim() }); });
$("source").addEventListener("change", () => applyFilters({ source: $("source").value }));
for (const id of ["reset", "empty-reset"]) $(id).addEventListener("click", () => applyFilters({ source: "", tag: "", q: "" }));
$("browse-tags").addEventListener("click", () => {
  $("tag-browser").hidden = !$("tag-browser").hidden;
  $("browse-tags").setAttribute("aria-expanded", String(!$("tag-browser").hidden));
  if (!$("tag-browser").hidden) {
    renderTagBrowser();
    // Groups unfold once, in order, when the index opens; typing a search re-renders without it.
    $("tag-browser").classList.remove("opening");
    void $("tag-browser").offsetWidth;
    $("tag-browser").classList.add("opening");
    $("tag-search").focus({ preventScroll: true });
  }
});
$("tag-search").addEventListener("input", renderTagBrowser);
$("tag-retry").addEventListener("click", () => { state.tagError = ""; renderTagBrowser(); loadTags(); });
$("retry").addEventListener("click", () => (state.months.length ? showPage(0) : loadHall()));
$("prev-page").addEventListener("click", () => turn(-1));
$("next-page").addEventListener("click", () => turn(1));
// Arrow keys turn pages when nothing else wants them; a horizontal swipe on the hall does the same.
document.addEventListener("keydown", (event) => {
  if ($("lightbox").open || event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return;
  if (event.target.closest?.("input, textarea, select, [contenteditable]")) return;
  if (event.key === "ArrowLeft") turn(-1);
  else if (event.key === "ArrowRight") turn(1);
});
// A finger drags the page sideways and lets go past 70px to turn it (with resistance where there is
// no page to turn to); short of that it springs back. Vertical strokes keep scrolling the page.
let hallSwipe = null;
function releaseSwipe(spread) {
  spread.style.transition = "translate .35s var(--ease), opacity .35s ease";
  spread.style.translate = "";
  spread.style.opacity = "";
}
$("grid").addEventListener("touchstart", (event) => {
  hallSwipe = event.touches.length === 1 && !state.loading ? { x: event.touches[0].clientX, y: event.touches[0].clientY, dx: 0, sideways: null } : null;
}, { passive: true });
$("grid").addEventListener("touchmove", (event) => {
  const spread = $("grid").children[0];
  if (!hallSwipe || !spread) return;
  const dx = event.touches[0].clientX - hallSwipe.x, dy = event.touches[0].clientY - hallSwipe.y;
  if (hallSwipe.sideways === null && Math.hypot(dx, dy) > 10) hallSwipe.sideways = Math.abs(dx) > Math.abs(dy);
  if (!hallSwipe.sideways) return;
  hallSwipe.dx = dx;
  const resist = neighbour(dx < 0 ? 1 : -1) ? .6 : .2;
  spread.style.transition = "none";
  spread.style.translate = `${dx * resist}px 0`;
  spread.style.opacity = String(1 - Math.min(Math.abs(dx) / 900, .35));
}, { passive: true });
function endSwipe() {
  const spread = $("grid").children[0], swipe = hallSwipe;
  hallSwipe = null;
  if (!swipe?.sideways || !spread) return;
  const step = swipe.dx < 0 ? 1 : -1;
  if (Math.abs(swipe.dx) > 70 && neighbour(step) && !state.loading) turn(step);
  else releaseSwipe(spread);
}
$("grid").addEventListener("touchend", endSwipe);
$("grid").addEventListener("touchcancel", endSwipe);
// A trackpad's two-finger sideways stroke turns one page per gesture, momentum included; taking the
// stroke here also keeps the browser's own back/forward swipe off the hall.
const wheel = { sum: 0, done: false, timer: 0 };
$("grid").addEventListener("wheel", (event) => {
  if (Math.abs(event.deltaX) <= Math.abs(event.deltaY)) return;
  event.preventDefault();
  clearTimeout(wheel.timer);
  wheel.timer = setTimeout(() => Object.assign(wheel, { sum: 0, done: false }), 250);
  if (wheel.done) return;
  wheel.sum += event.deltaX;
  if (Math.abs(wheel.sum) > 80) {
    wheel.done = true;
    turn(wheel.sum > 0 ? 1 : -1);
  }
}, { passive: false });
$("lb-close").addEventListener("click", closeLightbox);
$("lightbox").addEventListener("cancel", (event) => { event.preventDefault(); closeLightbox(); });
$("lightbox").addEventListener("close", () => {
  if ($("lightbox").open) return;
  state.closing = false;
  state.viewerRequest?.abort();
  state.viewerRequest = null;
  state.viewing = null;
  state.imageRequest = null; state.imageKey = null;
  $("lb-img").onload = $("lb-img").onerror = null;
  $("lb-img").removeAttribute("src");
  $("lb-preview").onload = $("lb-preview").onerror = null;
  $("lb-preview").removeAttribute("src");
  $("lb-thumbs").replaceChildren();
  syncFilters();
  if (!state.lastCard?.isConnected) $("collection").focus({ preventScroll: true });
});
$("lightbox").addEventListener("click", (event) => {
  if (event.target !== $("lightbox")) return;
  const rect = $("lightbox").getBoundingClientRect();
  if (event.clientX < rect.left || event.clientX > rect.right || event.clientY < rect.top || event.clientY > rect.bottom) closeLightbox();
});
$("lightbox").addEventListener("keydown", (event) => {
  if (event.target.matches("input, select, textarea")) return;
  if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
    event.preventDefault();
    showImage(state.imageIndex + (event.key === "ArrowLeft" ? -1 : 1));
  }
});
$("lb-prev").addEventListener("click", () => showImage(state.imageIndex - 1));
$("lb-next").addEventListener("click", () => showImage(state.imageIndex + 1));
$("lb-image-retry").addEventListener("click", () => showImage(state.imageIndex, true, true));
$("lb-retry").addEventListener("click", () => { if (state.viewing) openLightbox(state.viewing, state.lastCard, { push: false, page: state.imageIndex }); });
$("lb-share").addEventListener("click", async () => {
  if (!state.viewing) return;
  const url = new URL(galleryUrl(state.viewing.id, state.imageIndex), location.origin).href;
  try { await navigator.clipboard.writeText(url); $("lb-share").textContent = "已复制链接 ✓"; }
  catch { $("share-fallback").hidden = false; $("share-url").value = url; $("share-url").focus(); $("share-url").select(); }
});
let touchStart = null;
$("lb-img").addEventListener("touchstart", (event) => { touchStart = event.touches.length === 1 ? { x: event.touches[0].clientX, y: event.touches[0].clientY } : null; }, { passive: true });
$("lb-img").addEventListener("touchend", (event) => {
  if (!touchStart || !event.changedTouches.length) return;
  const dx = event.changedTouches[0].clientX - touchStart.x;
  const dy = event.changedTouches[0].clientY - touchStart.y;
  touchStart = null;
  if (Math.abs(dx) > 60 && Math.abs(dx) > Math.abs(dy) * 1.5) showImage(state.imageIndex + (dx < 0 ? 1 : -1));
}, { passive: true });
$("lb-img").addEventListener("touchcancel", () => { touchStart = null; });
$("about-open").addEventListener("click", () => $("about").showModal());
$("about-close").addEventListener("click", () => $("about").close());
// Touch feedback: a ring spreads like water from where a tag, tab or thumbnail is pressed, in the tag kind's colour.
const calm = matchMedia("(prefers-reduced-motion: reduce)");
document.addEventListener("pointerdown", (event) => {
  const target = event.target.closest?.(".tag-chip, .source-link, #browse-tags, .tag-kind, .featured-pick, .month-tab, .page-number, .pager-turn:not(:disabled)");
  if (!target || calm.matches) return;
  const box = target.getBoundingClientRect(), size = Math.max(box.width, box.height) * 2.4;
  const ripple = node("span", "ripple");
  ripple.style.cssText = `width:${size}px;height:${size}px;left:${event.clientX - box.left - size / 2}px;top:${event.clientY - box.top - size / 2}px`;
  ripple.addEventListener("animationend", () => ripple.remove());
  target.append(ripple);
});
// Cards lean toward the pointer with a sheen that follows it, like a print turned under a lamp.
if (matchMedia("(hover: hover) and (pointer: fine)").matches) {
  $("grid").addEventListener("pointermove", (event) => {
    const art = event.target.closest(".art-button");
    if (!art || calm.matches) return;
    const box = art.getBoundingClientRect(), x = (event.clientX - box.left) / box.width, y = (event.clientY - box.top) / box.height;
    art.style.setProperty("--tilt-x", `${((.5 - y) * 7).toFixed(2)}deg`);
    art.style.setProperty("--tilt-y", `${((x - .5) * 7).toFixed(2)}deg`);
    art.style.setProperty("--glare", `${(x * 100).toFixed(1)}% ${(y * 100).toFixed(1)}%`);
  });
  $("grid").addEventListener("pointerout", (event) => {
    const art = event.target.closest(".art-button");
    if (!art || art.contains(event.relatedTarget)) return;
    for (const name of ["--tilt-x", "--tilt-y", "--glare"]) art.style.removeProperty(name);
  });
}
addEventListener("resize", placeTabRule);
// The serif arrives after first layout and widens the tabs; measure the rule again once it has.
document.fonts?.ready.then(placeTabRule);
// Featured banner: five recent works, shown only on the unfiltered collection.
const featured = { works: [], index: 0, timer: 0 };
function showFeatured(index) {
  featured.index = (index + featured.works.length) % featured.works.length;
  $("featured-stage").querySelectorAll(".featured-slide").forEach((slide, i) => {
    slide.classList.toggle("active", i === featured.index);
    slide.setAttribute("aria-hidden", String(i !== featured.index));
    slide.tabIndex = i === featured.index ? 0 : -1;
  });
  $("featured-picker").querySelectorAll("button").forEach((pick, i) => {
    pick.classList.toggle("active", i === featured.index);
    pick.setAttribute("aria-pressed", String(i === featured.index));
  });
  const color = $("featured-stage").children[featured.index]?.dataset.color;
  if (color) $("featured").style.setProperty("--glow", color);
  stageColor();
}
// The page takes the colour of the artwork in view: the banner's slide while it fills the top of the
// screen, otherwise a card crossing the middle of the screen. Observers report both, so scrolling does
// no work of its own; the colour fades in on the hidden backdrop layer, so only opacity animates.
const stage = { cards: new Set(), banner: false, color: "", front: 0 };
function paintStage(color) {
  if (!color || color === stage.color) return;
  stage.color = color;
  const layers = document.querySelectorAll(".backdrop > span");
  if (layers.length < 2) return;
  stage.front ^= 1;
  const next = layers[stage.front], previous = layers[stage.front ^ 1];
  // A quick run of colours can leave both layers shown; start the incoming one from clear so it always fades.
  next.style.transition = "none";
  next.classList.remove("on");
  void next.offsetWidth;
  next.style.transition = "";
  next.style.setProperty("--stage", color);
  next.style.zIndex = "1";
  previous.style.zIndex = "0";
  next.classList.add("on");
  next.addEventListener("transitionend", () => { if (layers[stage.front] === next) previous.classList.remove("on"); }, { once: true });
}
function stageColor() {
  let color = "";
  if (stage.banner && !$("featured").hidden) color = $("featured-stage").children[featured.index]?.dataset.color;
  else for (const card of stage.cards) { color = card.style.getPropertyValue("--art"); if (color) break; }
  paintStage(color);
}
const middleObserver = typeof IntersectionObserver === "function" ? new IntersectionObserver((entries) => {
  for (const entry of entries) {
    if (entry.target.id === "featured") stage.banner = entry.isIntersecting;
    else if (entry.isIntersecting) stage.cards.add(entry.target);
    else stage.cards.delete(entry.target);
  }
  stageColor();
}, { rootMargin: "-40% 0px -40% 0px" }) : null;
middleObserver?.observe($("featured"));
function rotateFeatured(run) {
  clearInterval(featured.timer);
  if (run && featured.works.length > 1 && !matchMedia("(prefers-reduced-motion: reduce)").matches) {
    featured.timer = setInterval(() => showFeatured(featured.index + 1), 7000);
  }
}
function renderFeatured() {
  const visible = !state.source && !state.tag && !state.q;
  if (visible && !featured.works.length && state.works.length) {
    const pool = state.works.filter((work) => coverUrl(work.cover_url));
    // Wide-ish covers crop well into a banner; fall back to anything with a cover.
    const wide = pool.filter((work) => coverRatio(work.cover_width, work.cover_height) >= .9);
    featured.works = (wide.length >= 5 ? wide : pool).map((work) => [Math.random(), work])
      .sort((a, b) => a[0] - b[0]).slice(0, 5).map(([, work]) => work);
    $("featured-stage").replaceChildren(...featured.works.map((work) => {
      const title = work.title || work.source_id;
      const slide = node("a", "featured-slide");
      slide.href = galleryUrl(work.id);
      slide.setAttribute("aria-label", `查看 ${title}`);
      const image = node("img");
      image.src = thumbUrl(work.cover_url, 1280);
      image.alt = "";
      image.decoding = "async";
      image.onload = () => {
        slide.dataset.color = artColor(image);
        if (slide.classList.contains("active")) { $("featured").style.setProperty("--glow", slide.dataset.color); stageColor(); }
      };
      const caption = node("div", "featured-caption");
      caption.append(node("p", "featured-source", names[work.source] || work.source),
        node("h2", "featured-title", title), node("p", "featured-author", work.author_name || "未署名"));
      slide.append(image, caption);
      slide.addEventListener("click", (event) => {
        if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
        event.preventDefault();
        openLightbox(work, slide);
      });
      return slide;
    }));
    $("featured-picker").replaceChildren(...featured.works.map((work, i) => {
      const pick = button("featured-pick", undefined, () => { showFeatured(i); rotateFeatured(true); });
      pick.setAttribute("aria-label", `第 ${i + 1} 幅：${work.title || work.source_id}`);
      const thumb = node("img");
      thumb.src = thumbUrl(work.cover_url, 160);
      thumb.alt = "";
      pick.append(thumb);
      return pick;
    }));
    showFeatured(0);
  }
  $("featured").hidden = !visible || !featured.works.length;
  stageColor();
  rotateFeatured(!$("featured").hidden);
}
for (const [type, run] of [["pointerenter", false], ["pointerleave", true], ["focusin", false], ["focusout", true]]) {
  $("featured").addEventListener(type, () => rotateFeatured(run && !$("featured").hidden));
}
// The brush-script title is drawn on, left to right, whenever the collection changes.
let drawnTitle = "";
new MutationObserver(() => {
  if ($("collection-title").textContent === drawnTitle) return;
  drawnTitle = $("collection-title").textContent;
  if (!matchMedia("(prefers-reduced-motion: reduce)").matches) $("collection-title").animate([
    { clipPath: "inset(-10% 100% -10% 0)", opacity: .4 }, { clipPath: "inset(-10% 0 -10% 0)", opacity: 1 },
  ], { duration: 700, easing: "cubic-bezier(.65, 0, .35, 1)" });
}).observe($("collection-title"), { childList: true, characterData: true, subtree: true });
window.addEventListener("popstate", syncRoute);
syncFilters();
loadHall();
loadFilters();
syncRoute();

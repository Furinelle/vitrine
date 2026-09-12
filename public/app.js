const $ = (id) => document.getElementById(id);
const names = { pixiv: "Pixiv", x: "X / Twitter", douyin: "抖音" };
const symbols = { pixiv: "P", x: "𝕏", douyin: "♪" };
const initial = new URLSearchParams(location.search);
const state = {
  source: initial.get("source") || "", tag: initial.get("tag") || "", q: initial.get("q") || "",
  works: [], offset: 0, limit: 24, done: false, loading: false, request: null,
  sources: [], viewerRequest: null, viewing: null, images: [], imageIndex: 0,
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
    return ["https:", "http:"].includes(url.protocol) ? url.href : "";
  } catch { return ""; }
}

function mediaUrl(key) {
  return "/media/" + String(key).split("/").map(encodeURIComponent).join("/");
}

function coverUrl(value) {
  try {
    const url = new URL(value, location.origin);
    return url.origin === location.origin && url.pathname.startsWith("/media/") ? url.href : "";
  } catch { return ""; }
}

async function getJson(url, signal) {
  const response = await fetch(url, { signal });
  if (!response.ok) throw new Error(`请求失败（${response.status}）`);
  const data = await response.json();
  if (!data.ok) throw new Error("暂时无法读取图库");
  return data;
}

function syncFilters(preserveDraft = false) {
  $("source").value = state.source;
  if (!preserveDraft) {
    $("tag").value = state.tag;
    $("q").value = state.q;
  }
  document.querySelectorAll(".source-link").forEach((item) => {
    const active = item.dataset.source === state.source;
    item.classList.toggle("active", active);
    if (active) item.setAttribute("aria-current", "page");
    else item.removeAttribute("aria-current");
  });
  document.querySelectorAll("#popular-tags .tag-chip").forEach((item) => {
    item.classList.toggle("active", item.dataset.tag === state.tag);
  });
  $("current-source").textContent = names[state.source] || state.source || "全部作品";
  $("collection-title").replaceChildren(document.createTextNode(state.tag ? `# ${state.tag}` : state.q ? "搜索结果" : state.source ? names[state.source] || state.source : "所有作品"), node("span", "heading-dot", "."));
  $("reset").hidden = !state.source && !state.tag && !state.q;
  $("active-filters").replaceChildren();
  for (const field of ["tag", "q"]) {
    if (!state[field]) continue;
    const chip = button("active-filter", `${field === "tag" ? "# " : "搜索："}${state[field]} ×`, () => applyFilters({ [field]: "" }));
    chip.setAttribute("aria-label", `移除${field === "tag" ? "标签" : "搜索"}筛选：${state[field]}`);
    $("active-filters").append(chip);
  }
}

function updateCount() {
  const total = state.source ? state.sources.find((s) => s.source === state.source)?.cnt : state.sources.length ? state.sources.reduce((sum, s) => sum + s.cnt, 0) : null;
  $("collection-count").textContent = !state.tag && !state.q && total != null ? `${total.toLocaleString("zh-CN")} 件收藏` : `已载入 ${state.works.length.toLocaleString("zh-CN")} 件作品`;
}

function applyFilters(changes) {
  Object.assign(state, changes);
  const query = new URLSearchParams();
  for (const field of ["source", "tag", "q"]) if (state[field]) query.set(field, state[field]);
  history.replaceState(null, "", location.pathname + (query.size ? `?${query}` : ""));
  syncFilters();
  loadWorks(true);
}

async function loadFilters() {
  const results = await Promise.allSettled([getJson("/api/sources"), getJson("/api/tags")]);
  if (results[0].status === "fulfilled") {
    state.sources = results[0].value.sources || [];
    $("sources").replaceChildren();
    $("source").replaceChildren(new Option("全部来源", ""));
    const all = [{ source: "", cnt: state.sources.reduce((sum, s) => sum + s.cnt, 0) }, ...state.sources];
    for (const source of all) {
      const label = names[source.source] || source.source || "全部作品";
      const item = button("source-link", undefined, () => applyFilters({ source: source.source }));
      item.dataset.source = source.source;
      const symbol = node("span", "source-symbol", symbols[source.source] || "▦");
      symbol.setAttribute("aria-hidden", "true");
      item.append(symbol, node("span", "", label), node("span", "source-count", source.cnt.toLocaleString("zh-CN")));
      $("sources").append(item);
      if (source.source) $("source").append(new Option(label, source.source));
    }
    if (state.source && !state.sources.some((s) => s.source === state.source)) $("source").append(new Option(state.source, state.source));
  }
  if (results[1].status === "fulfilled") {
    const tags = results[1].value.tags || [];
    $("taglist").replaceChildren(...tags.map((tag) => new Option(`${tag.name} · ${tag.cnt} 件作品`, tag.name)));
    $("popular-tags").replaceChildren(...tags.slice(0, 9).map((tag) => tagButton(tag.name)));
  }
  syncFilters(true);
  updateCount();
}

function tagButton(tag) {
  const chip = button("tag-chip", `# ${tag}`, () => {
    if ($("lightbox").open) $("lightbox").close();
    applyFilters({ tag });
  });
  chip.dataset.tag = tag;
  return chip;
}

function renderCards(works, append) {
  if (!append) $("grid").replaceChildren();
  const fragment = document.createDocumentFragment();
  for (const work of works) {
    const title = work.title || work.source_id;
    const card = node("article", "card");
    const art = button("art-button", undefined, () => openLightbox(work));
    art.setAttribute("aria-label", `查看 ${title}，${work.page_count} 张图片`);
    const image = node("img");
    image.alt = title;
    image.loading = "lazy";
    image.decoding = "async";
    image.addEventListener("error", () => art.classList.add("image-broken"));
    const url = coverUrl(work.cover_url);
    if (url) image.src = url;
    else art.classList.add("image-broken");
    art.append(image, node("span", "image-fallback", "图片暂不可用"));
    if (work.page_count > 1) art.append(node("span", "image-badge", `${work.page_count} 张`));
    if (work.is_r18) art.append(node("span", "r18-badge", "R18"));
    const body = node("div", "card-body");
    const heading = node("h2", "card-title", title);
    heading.title = title;
    const meta = node("div", "card-meta");
    meta.append(node("span", "card-author", work.author_name || "未署名"), node("span", "card-source", (names[work.source] || work.source).toUpperCase()));
    const tags = node("div", "card-tags");
    for (const tag of (work.tags || []).slice(0, 2)) tags.append(button("", `#${tag}`, () => applyFilters({ tag })));
    body.append(heading, meta, tags);
    card.append(art, body);
    fragment.append(card);
  }
  $("grid").append(fragment);
}

async function loadWorks(reset = false) {
  if (!reset && (state.loading || state.done)) return;
  if (reset) {
    state.request?.abort();
    state.offset = 0;
    state.done = false;
    state.works = [];
    $("grid").replaceChildren();
    updateCount();
  }
  const controller = new AbortController();
  state.request = controller;
  state.loading = true;
  $("grid").setAttribute("aria-busy", "true");
  $("status").textContent = reset ? "正在加载作品…" : "正在加载更多作品…";
  $("status").classList.remove("error");
  $("retry").hidden = true;
  $("empty").hidden = true;
  $("end-note").hidden = true;
  $("more").disabled = true;
  const query = new URLSearchParams({ limit: state.limit, offset: state.offset });
  for (const field of ["source", "tag", "q"]) if (state[field]) query.set(field, state[field]);
  try {
    const data = await getJson(`/api/works?${query}`, controller.signal);
    if (state.request !== controller) return;
    const works = data.works || [];
    state.works.push(...works);
    renderCards(works, !reset);
    state.offset += works.length;
    state.done = works.length < state.limit;
    $("status").textContent = "";
    $("empty").hidden = state.works.length > 0;
    $("end-note").hidden = !state.done || !state.works.length;
    $("more").hidden = state.done;
    updateCount();
  } catch (error) {
    if (controller.signal.aborted || state.request !== controller) return;
    $("status").textContent = `${error.message}，请稍后重试。`;
    $("status").classList.add("error");
    $("retry").hidden = false;
    $("more").hidden = true;
  } finally {
    if (state.request === controller) {
      state.loading = false;
      $("grid").setAttribute("aria-busy", "false");
      $("more").disabled = false;
    }
  }
}

function showImage(index) {
  if (!state.images.length) return;
  state.imageIndex = Math.max(0, Math.min(index, state.images.length - 1));
  const image = state.images[state.imageIndex];
  const url = image.r2_key ? mediaUrl(image.r2_key) : coverUrl(image.cover_url);
  $("lb-img").hidden = false;
  $("lb-image-error").hidden = true;
  $("lb-img").src = url;
  $("lb-img").alt = `${state.viewing.title || state.viewing.source_id} · 第 ${state.imageIndex + 1} 张`;
  $("lb-original").href = url;
  $("lb-counter").textContent = `${state.imageIndex + 1} / ${state.images.length}`;
  $("lb-prev").disabled = state.imageIndex === 0;
  $("lb-next").disabled = state.imageIndex === state.images.length - 1;
  $("lb-prev").hidden = $("lb-next").hidden = state.images.length < 2;
  $("lb-thumbs").querySelectorAll("button").forEach((thumb, i) => {
    thumb.classList.toggle("active", i === state.imageIndex);
    thumb.setAttribute("aria-pressed", String(i === state.imageIndex));
  });
}

async function openLightbox(work) {
  state.viewerRequest?.abort();
  const controller = new AbortController();
  state.viewerRequest = controller;
  state.viewing = work;
  state.images = [{ cover_url: work.cover_url }];
  $("lb-title").textContent = work.title || work.source_id;
  $("lb-source").textContent = (names[work.source] || work.source).toUpperCase();
  $("lb-author").textContent = work.author_name || "未署名";
  const date = new Date(work.created_at);
  $("lb-info").textContent = `${work.page_count} 张图片${work.is_r18 ? " · R18" : ""}${Number.isNaN(date.valueOf()) ? "" : ` · 收藏于 ${date.toLocaleDateString("zh-CN")}`}`;
  $("lb-tags").replaceChildren(...(work.tags || []).map(tagButton));
  $("lb-thumbs").replaceChildren();
  $("lb-status").textContent = "正在加载完整作品…";
  const source = safeLink(work.source_url);
  $("lb-link").hidden = !source;
  if (source) $("lb-link").href = source;
  else $("lb-link").removeAttribute("href");
  showImage(0);
  if (!$("lightbox").open) $("lightbox").showModal();
  try {
    const data = await getJson(`/api/works/${encodeURIComponent(work.id)}`, controller.signal);
    if (state.viewerRequest !== controller) return;
    if (!data.images?.length) throw new Error("作品暂时不可用");
    state.images = data.images;
    $("lb-status").textContent = "";
    if (state.images.length > 1) {
      $("lb-thumbs").replaceChildren(...state.images.map((image, i) => {
        const thumb = button("thumb", undefined, () => showImage(i));
        thumb.setAttribute("aria-label", `查看第 ${i + 1} 张图片`);
        const img = node("img");
        img.src = mediaUrl(image.r2_key);
        img.alt = "";
        img.loading = "lazy";
        thumb.append(img);
        return thumb;
      }));
    }
    showImage(0);
  } catch (error) {
    if (controller.signal.aborted || state.viewerRequest !== controller) return;
    $("lb-status").textContent = `${error.message}。可关闭后重新打开。`;
    if (!state.images[0]?.r2_key) {
      $("lb-counter").textContent = "封面预览";
      $("lb-prev").hidden = $("lb-next").hidden = true;
    }
  }
}

$("source").value = state.source;
$("tag").value = state.tag;
$("q").value = state.q;
$("search-form").addEventListener("submit", (event) => { event.preventDefault(); applyFilters({ q: $("q").value.trim() }); });
$("filter-form").addEventListener("submit", (event) => { event.preventDefault(); applyFilters({ source: $("source").value, tag: $("tag").value.trim(), q: $("q").value.trim() }); });
$("source").addEventListener("change", () => applyFilters({ source: $("source").value }));
for (const id of ["reset", "empty-reset"]) $(id).addEventListener("click", () => applyFilters({ source: "", tag: "", q: "" }));
$("browse-tags").addEventListener("click", () => $("tag").focus());
$("more").addEventListener("click", () => loadWorks());
$("retry").addEventListener("click", () => loadWorks(state.offset === 0));
$("lb-close").addEventListener("click", () => $("lightbox").close());
$("lightbox").addEventListener("close", () => { state.viewerRequest?.abort(); $("lb-img").removeAttribute("src"); });
$("lightbox").addEventListener("click", (event) => {
  if (event.target !== $("lightbox")) return;
  const rect = $("lightbox").getBoundingClientRect();
  if (event.clientX < rect.left || event.clientX > rect.right || event.clientY < rect.top || event.clientY > rect.bottom) $("lightbox").close();
});
$("lightbox").addEventListener("keydown", (event) => {
  if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
    event.preventDefault();
    showImage(state.imageIndex + (event.key === "ArrowLeft" ? -1 : 1));
  }
});
$("lb-prev").addEventListener("click", () => showImage(state.imageIndex - 1));
$("lb-next").addEventListener("click", () => showImage(state.imageIndex + 1));
$("lb-img").addEventListener("error", () => { $("lb-img").hidden = true; $("lb-image-error").hidden = false; });
syncFilters();
loadWorks(true);
loadFilters();

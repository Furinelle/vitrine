const $ = (id) => document.getElementById(id);
const names = { pixiv: "Pixiv", x: "X / Twitter", douyin: "抖音" };
// tag 可以是逗号分隔的多个标签（命中任一即可）；界面上只写第一个和总数。
function tagLabel(tag) {
  const tags = tag.split(",").filter(Boolean);
  return tags.length > 1 ? `${tags[0]} 等 ${tags.length} 个标签` : tag;
}
function locationFilters() {
  const query = new URLSearchParams(location.search);
  return { source: query.get("source") || "", tag: query.get("tag") || "", q: query.get("q") || "" };
}
const state = {
  ...locationFilters(), works: [], offset: 0, cursor: null, limit: 24, done: false, loading: false,
  request: null, sources: [], tags: [], viewerRequest: null, viewing: null, images: [], imageIndex: 0,
  tagRequest: null, tagLoading: false, tagOffset: 0, tagNext: null,
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
  for (const field of ["source", "tag", "q"]) if (state[field]) query.set(field, state[field]);
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
function updateCount() {
  const total = state.source ? state.sources.find((s) => s.source === state.source)?.cnt : state.sources.length ? state.sources.reduce((sum, s) => sum + s.cnt, 0) : null;
  $("collection-count").textContent = !state.tag && !state.q && total != null ? `${total.toLocaleString("zh-CN")} 件收藏` : `已载入 ${state.works.length.toLocaleString("zh-CN")} 件作品`;
}
function applyFilters(changes) {
  Object.assign(state, changes);
  if ($("lightbox").open) {
    state.viewerRequest?.abort(); state.viewerRequest = null;
    dismissLightbox();
  }
  history.pushState(null, "", galleryUrl());
  syncFilters();
  loadWorks(true);
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
    if (state.source && !state.sources.some((s) => s.source === state.source)) $("source").append(new Option(state.source, state.source));
    syncFilters(true);
    updateCount();
  }), getJson("/api/tags").then((data) => {
    state.tags = data.tags || [];
    $("taglist").replaceChildren(...state.tags.map((tag) => new Option(`${tag.name} · ${tag.cnt} 件作品`, tag.name)));
    $("popular-tags").replaceChildren(...state.tags.slice(0, 8).map((tag) => tagButton(tag.name)));
    syncFilters(true);
  })]);
}
async function loadTagBrowser(reset = true) {
  if (!reset && (state.tagLoading || state.tagNext === null)) return;
  state.tagRequest?.abort();
  const controller = new AbortController();
  state.tagRequest = controller;
  state.tagLoading = true;
  if (reset) { state.tagOffset = 0; $("all-tags").replaceChildren(); }
  $("tag-status").textContent = "加载标签…";
  $("tag-retry").hidden = true;
  $("tag-more").hidden = true;
  $("tag-empty").hidden = true;
  const query = new URLSearchParams({ q: $("tag-search").value.trim(), limit: 100, offset: state.tagOffset, order: "name" });
  try {
    const data = await getJson(`/api/tags?${query}`, controller.signal);
    if (state.tagRequest !== controller) return;
    $("all-tags").append(...data.tags.map((tag) => {
      const chip = tagButton(tag.name);
      chip.title = `${tag.cnt} 件作品`;
      return chip;
    }));
    state.tagNext = data.next_offset;
    state.tagOffset += data.tags.length;
    $("tag-more").hidden = state.tagNext === null;
    $("tag-empty").hidden = state.tagOffset > 0;
    $("tag-status").textContent = "";
  } catch (error) {
    if (controller.signal.aborted || state.tagRequest !== controller) return;
    $("tag-status").textContent = `${error.message}。`;
    $("tag-retry").hidden = false;
  } finally {
    if (state.tagRequest === controller) state.tagLoading = false;
  }
}
function tagButton(tag) {
  const chip = button("tag-chip", `# ${tag}`, async () => {
    const fromDialog = $("lightbox").open;
    if (fromDialog) await dismissLightbox();
    applyFilters({ tag });
    if (fromDialog) $("collection").focus();
  });
  chip.dataset.tag = tag;
  return chip;
}
// Native lazy-loading can fetch several screens ahead; keep gallery traffic near the viewport.
const coverObserver = typeof IntersectionObserver === "function" ? new IntersectionObserver((entries) => {
  for (const { target: image, isIntersecting } of entries) {
    if (!isIntersecting || !image.isConnected || !image.dataset.src) continue;
    image.src = image.dataset.src;
    delete image.dataset.src;
    coverObserver.unobserve(image);
  }
}, { rootMargin: "360px 0px" }) : null;
// Extreme strips and panoramas are cropped so one cover never dominates a column.
function coverRatio(width, height) {
  return width > 0 && height > 0 ? Math.min(1.8, Math.max(.5, width / height)) : 0;
}
function renderLoadingCards() {
  coverObserver?.disconnect();
  $("grid").replaceChildren(...Array.from({ length: 8 }, () => {
    const card = node("div", "card-skeleton");
    card.setAttribute("aria-hidden", "true");
    card.append(node("div", "skeleton-art"), node("div", "skeleton-line"), node("div", "skeleton-line short"));
    return card;
  }));
}
function renderCards(works, append) {
  if (!append) $("grid").replaceChildren();
  const fragment = document.createDocumentFragment();
  for (const [i, work] of works.entries()) {
    const title = work.title || work.source_id;
    const card = node("article", "card");
    card.dataset.workId = work.id;
    card.style.animationDelay = `${(i % 12) * 40}ms`;
    const art = node("a", "art-button");
    const ratio = coverRatio(work.cover_width, work.cover_height);
    art.style.aspectRatio = String(ratio || .75);
    art.href = galleryUrl(work.id);
    art.setAttribute("aria-label", `查看 ${title}，${work.page_count} 张图片`);
    const image = node("img");
    image.alt = title;
    image.loading = !append && i < 4 ? "eager" : "lazy";
    image.decoding = "async";
    if (!append && i < 2) image.fetchPriority = "high";
    const original = coverUrl(work.cover_url);
    image.onload = () => {
      if (!ratio) art.style.aspectRatio = String(coverRatio(image.naturalWidth, image.naturalHeight) || .75);
      art.classList.add("image-loaded");
    };
    image.onerror = () => art.classList.add("image-broken");
    if (original) {
      if (image.loading === "eager" || !coverObserver) image.src = thumbUrl(original);
      else { image.dataset.src = thumbUrl(original); coverObserver.observe(image); }
    } else art.classList.add("image-broken");
    art.append(image, node("span", "image-fallback", "预览暂不可用 · 查看详情"));
    if (work.page_count > 1) art.append(node("span", "image-badge", `${work.page_count} 张`));
    const body = node("div", "card-body");
    const heading = node("h2", "card-title");
    const titleLink = node("a", "", title);
    titleLink.href = art.href;
    titleLink.title = title;
    heading.append(titleLink);
    for (const link of [art, titleLink]) link.addEventListener("click", (event) => {
      if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
      event.preventDefault();
      openLightbox(work, card);
    });
    const meta = node("div", "card-meta");
    meta.append(node("span", "card-author", work.author_name || "未署名"), node("span", "card-source", names[work.source] || work.source));
    const tags = node("div", "card-tags");
    for (const tag of (work.tags || []).slice(0, 2)) tags.append(button("", `#${tag}`, () => applyFilters({ tag })));
    body.append(heading, meta, tags);
    card.append(art, body);
    fragment.append(card);
  }
  $("grid").append(fragment);
}
const moreObserver = typeof IntersectionObserver === "function" ? new IntersectionObserver((entries) => {
  if (entries.some((entry) => entry.isIntersecting) && !$("more").hidden) return loadWorks();
}, { rootMargin: "0px 0px 400px 0px" }) : null;
async function loadWorks(reset = false) {
  if (!reset && (state.loading || state.done)) return;
  moreObserver?.unobserve($("more"));
  if (reset) {
    state.request?.abort();
    state.offset = 0;
    state.cursor = null;
    state.done = false;
    state.works = [];
    renderLoadingCards();
    updateCount();
  }
  const controller = new AbortController();
  state.request = controller;
  state.loading = true;
  $("grid").setAttribute("aria-busy", "true");
  $("status").textContent = reset ? "正在加载作品…" : "";
  $("status").classList.remove("error");
  $("retry").hidden = true;
  $("empty").hidden = true;
  $("end-note").hidden = true;
  $("more").disabled = true;
  $("more").textContent = "正在加载…";
  const query = new URLSearchParams({ limit: state.limit });
  if (state.cursor) query.set("cursor", state.cursor);
  else query.set("offset", state.offset);
  for (const field of ["source", "tag", "q"]) if (state[field]) query.set(field, state[field]);
  try {
    const data = await getJson(`/api/works?${query}`, controller.signal);
    if (state.request !== controller) return;
    const works = data.works || [];
    const existing = new Set(state.works.map((work) => work.id));
    const fresh = works.filter((work) => !existing.has(work.id));
    state.works.push(...fresh);
    renderCards(fresh, !reset);
    state.offset += works.length;
    state.cursor = data.next_cursor ?? null;
    state.done = "next_cursor" in data ? !state.cursor : works.length < state.limit;
    $("status").textContent = "";
    $("empty").hidden = state.works.length > 0;
    $("end-note").hidden = !state.done || !state.works.length;
    $("more").hidden = state.done;
    updateCount();
  } catch (error) {
    if (controller.signal.aborted || state.request !== controller) return;
    $("status").textContent = `${error.message}。`;
    $("status").classList.add("error");
    if (!state.works.length) $("grid").replaceChildren();
    $("retry").hidden = false;
    $("more").hidden = true;
  } finally {
    if (state.request === controller) {
      state.loading = false;
      $("grid").setAttribute("aria-busy", "false");
      $("more").disabled = false;
      $("more").textContent = "继续浏览 ↓";
      if (!$("more").hidden) moreObserver?.observe($("more"));
    }
  }
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
  const small = { opacity: 0, transform: "translateY(18px) scale(.95)" };
  const motion = dialog.animate(closing ? [current, small] : [small, rest], {
    duration: closing ? 230 : 360, easing: "cubic-bezier(.22, 1, .36, 1)", fill: "both",
  });
  state.viewerMotion = motion;
  if (!closing) motion.finished.then(() => {
    if (state.viewerMotion === motion) { motion.cancel(); state.viewerMotion = null; }
  }, () => {});
  return motion;
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
  if (entering) {
    const card = cardEl?.getBoundingClientRect();
    const frame = dialog.getBoundingClientRect();
    dialog.style.transformOrigin = card ? `${Math.max(0, Math.min(100, (card.x + card.width / 2 - frame.x) / frame.width * 100))}% ${Math.max(0, Math.min(100, (card.y + card.height / 2 - frame.y) / frame.height * 100))}%` : "50% 60%";
    animateLightbox();
  }
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
  Object.assign(state, filters);
  if (changed) { syncFilters(); loadWorks(true); }
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
  if (!$("tag-browser").hidden) { $("tag-search").focus(); loadTagBrowser(); }
  else { clearTimeout(tagSearchTimer); state.tagRequest?.abort(); }
});
let tagSearchTimer;
$("tag-search").addEventListener("input", () => {
  clearTimeout(tagSearchTimer);
  state.tagRequest?.abort();
  $("tag-more").hidden = true;
  tagSearchTimer = setTimeout(() => loadTagBrowser(), 200);
});
$("tag-more").addEventListener("click", () => loadTagBrowser(false));
$("tag-retry").addEventListener("click", () => loadTagBrowser(state.tagOffset === 0));
$("more").addEventListener("click", () => loadWorks());
$("retry").addEventListener("click", () => loadWorks(state.offset === 0));
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
if (!matchMedia("(prefers-reduced-motion: reduce)").matches && matchMedia("(pointer: fine)").matches) {
  for (const glass of document.querySelectorAll(".liquid-glass")) glass.addEventListener("pointermove", (event) => {
    const rect = glass.getBoundingClientRect();
    glass.style.setProperty("--shine-x", `${Math.round((event.clientX - rect.left) / rect.width * 100)}%`);
  }, { passive: true });
}
// Masonry: cards keep DOM (and keyboard) order; each is translated into the shortest column.
if (typeof ResizeObserver === "function") {
  const grid = $("grid");
  let frame = 0;
  const layout = () => {
    frame = 0;
    const items = [...grid.children];
    const style = getComputedStyle(grid);
    const columns = Math.max(1, parseInt(style.getPropertyValue("--columns"), 10) || 2);
    const gapX = parseFloat(style.columnGap) || 0, gapY = parseFloat(style.rowGap) || 0;
    const width = (grid.clientWidth - gapX * (columns - 1)) / columns;
    grid.style.setProperty("--card-width", `${width}px`);
    const heights = items.map((item) => item.offsetHeight);
    const bottoms = Array(columns).fill(0);
    const fresh = [];
    items.forEach((item, i) => {
      const column = bottoms.indexOf(Math.min(...bottoms));
      item.style.translate = `${column * (width + gapX)}px ${bottoms[column]}px`;
      bottoms[column] += heights[i] + gapY;
      if (!item.classList.contains("placed")) fresh.push(item);
    });
    grid.style.height = items.length ? `${Math.max(...bottoms) - gapY}px` : "";
    // Commit first positions before enabling the slide used for later reflows.
    if (fresh.length) { void grid.offsetWidth; for (const item of fresh) item.classList.add("placed"); }
  };
  const schedule = () => { frame ||= requestAnimationFrame(layout); };
  const sizes = new ResizeObserver(schedule);
  new MutationObserver((records) => {
    for (const record of records) {
      for (const item of record.addedNodes) if (item.nodeType === 1) sizes.observe(item);
      for (const item of record.removedNodes) if (item.nodeType === 1) sizes.unobserve(item);
    }
    schedule();
  }).observe(grid, { childList: true });
  sizes.observe(grid);
  grid.classList.add("masonry");
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
loadWorks(true);
loadFilters();
syncRoute();

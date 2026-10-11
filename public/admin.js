const $ = (id) => document.getElementById(id);
const names = { pixiv: "Pixiv", x: "X / Twitter", douyin: "抖音" };

function parseTags(raw) {
  const out = [];
  for (const part of String(raw || "").split(/[,，]+/)) {
    const tag = part.trim().replace(/^#/, "").trim();
    if (tag && !out.includes(tag)) out.push(tag);
  }
  return out;
}

function workApiPath(id, suffix = "") {
  return "/api/admin/works/" + encodeURIComponent(id) + suffix;
}

function node(tag, className, text) {
  const element = document.createElement(tag);
  if (className) element.className = className;
  if (text !== undefined) element.textContent = text;
  return element;
}

function safeLink(value) {
  try {
    const url = new URL(value);
    return ["https:", "http:"].includes(url.protocol) ? url.href : "";
  } catch {
    return "";
  }
}

function mediaUrl(key) {
  return "/media/" + String(key).split("/").map(encodeURIComponent).join("/");
}

function coverUrl(value) {
  try {
    const url = new URL(value, location.origin);
    return url.origin === location.origin && url.pathname.startsWith("/media/") ? url.href : "";
  } catch {
    return "";
  }
}

function thumbUrl(value, width = 320) {
  try {
    const url = new URL(value, location.origin);
    return url.origin === location.origin && url.pathname.startsWith("/media/")
      ? `${url.origin}${url.pathname}?width=${width <= 160 ? 160 : width <= 640 ? 640 : 1280}`
      : "";
  } catch {
    return "";
  }
}

async function send(url, options = {}) {
  const response = await fetch(url, { credentials: "same-origin", cache: "no-store", ...options });
  const data = await response.json().catch(() => ({ ok: false, error: response.status === 429 ? "请求太频繁，请稍后再试。" : "暂时无法加载，请稍后重试。" }));
  if (!response.ok || data.ok === false) {
    const error = new Error(response.status === 401 ? "unauthorized" : data.error || `请求失败（${response.status}）`);
    error.status = response.status;
    error.code = data.code;
    error.retryAfter = Number(response.headers.get("Retry-After")) || 0;
    throw error;
  }
  return data;
}

const state = {
  source: "",
  q: "",
  workStatus: "active",
  listGeneration: 0,
  works: [],
  offset: 0,
  limit: 24,
  done: false,
  loading: false,
  viewing: null,
  savedEdit: "",
  editorLoading: false,
  workBusy: false,
  images: [],
  decisions: {},
  similar: [],
  autoPairs: [],
  view: "works",
  similarRetry: readSimilarRetry(),
  similarDeleting: false,
};

function readSimilarRetry() {
  try {
    const items = JSON.parse(sessionStorage.getItem("vitrine-similar-retry") || "[]");
    return Array.isArray(items) ? items.filter((item) => typeof item.r2_key === "string" &&
      /^sim-[0-9a-f-]{36}$/.test(item.decision_id) && (!item.keep_r2_keys ||
      Array.isArray(item.keep_r2_keys) && item.keep_r2_keys.length <= 20 &&
      item.keep_r2_keys.every((key) => typeof key === "string" && key.length > 0 && key.length <= 1024))).slice(0, 1000) : [];
  } catch {
    return [];
  }
}

function saveSimilarRetry() {
  try {
    sessionStorage.setItem("vitrine-similar-retry", JSON.stringify(state.similarRetry));
  } catch { /* In-memory retry remains available if browser storage is disabled. */ }
  let button = $("similar-retry");
  if (!button) {
    button = node("button", "text-button", "重试未完成的删除");
    button.id = "similar-retry";
    button.type = "button";
    button.addEventListener("click", () => deleteSimilarItems(state.similarRetry.slice(0, 100)));
    $("similar-refresh")?.after(button);
  }
  button.hidden = !state.similarRetry.length;
  button.disabled = state.similarDeleting;
  let cancel = $("similar-retry-cancel");
  if (!cancel) {
    cancel = node("button", "text-button", "取消未完成操作");
    cancel.id = "similar-retry-cancel";
    cancel.type = "button";
    cancel.addEventListener("click", () => {
      if (!confirm("取消保存的重试操作？已经删除的图片不会恢复，请先核实结果。")) return;
      state.similarRetry = [];
      saveSimilarRetry();
      $("similar-status").textContent = "已取消重试，可刷新后重新选择未删除的图片。";
    });
    button.after(cancel);
  }
  cancel.hidden = button.hidden;
  cancel.disabled = button.disabled;
}

function similarDeleteItems(keys, keepers) {
  return [...new Set(keys)].map((r2_key) => state.similarRetry.find((item) => item.r2_key === r2_key) ||
    { r2_key, decision_id: `sim-${crypto.randomUUID()}`, keep_r2_keys: [...new Set(keepers(r2_key))] });
}

async function deleteSimilarItems(items) {
  if (!items.length || state.similarDeleting) return;
  state.similarDeleting = true;
  const byKey = new Map(state.similarRetry.map((item) => [item.r2_key, item]));
  for (const item of items) byKey.set(item.r2_key, item);
  state.similarRetry = [...byKey.values()];
  saveSimilarRetry();
  $("similar-status").textContent = `正在删除 ${items.length} 张…`;
  let message;
  try {
    const data = await api("/api/admin/similar/delete", {
      method: "POST", headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ items }),
    });
    const deleted = new Set(data.deleted || []);
    state.similarRetry = state.similarRetry.filter((item) => !deleted.has(item.r2_key));
    const failures = data.failed || [];
    const channelFailed = data.telegram_failed || [];
    const pending = data.telegram_pending || [];
    message = `图库已删除 ${deleted.size} 张；${failures.length} 张未完成。`;
    if (channelFailed.length) message += ` ${channelFailed.length} 条频道消息删除失败，需人工核实：${channelFailed.map((target) => `${target.chat_id}/${target.message_id}`).join("，")}。`;
    if (pending.length) message += ` ${pending.length} 条频道消息状态未确认，需人工核实：${pending.map((target) => `${target.chat_id}/${target.message_id}`).join("，")}。`;
    if (failures.length) message += ` ${failures.map((item) => `${item.r2_key}: ${item.error}`).join("；")}`;
    await loadSimilar();
  } catch (error) {
    if (error.status === 401) return;
    message = `未确认删除结果，已保留操作 ID，可重试：${error.message}`;
  } finally {
    state.similarDeleting = false;
    saveSimilarRetry();
    if (message) $("similar-status").textContent = message;
  }
}

function showLogin(message = "") {
  $("login").hidden = false;
  $("console").hidden = true;
  $("login-status").textContent = message;
}

function showConsole() {
  $("login").hidden = true;
  $("console").hidden = false;
}

async function api(url, options) {
  try {
    return await send(url, options);
  } catch (error) {
    if (error.status === 401) {
      showLogin("登录已失效，请重新输入管理密钥。");
    }
    throw error;
  }
}

function renderList(works, append) {
  if (!append) $("grid").replaceChildren();
  const fragment = document.createDocumentFragment();
  for (const work of works) {
    const title = work.title || work.source_id;
    const card = node("button", "admin-card glass-inset glass-inset-hover");
    card.type = "button";
    card.dataset.id = work.id;
    card.addEventListener("click", () => openEditor(work));

    const thumbWrap = node("div", "admin-card-thumb");
    const img = node("img");
    img.alt = title;
    img.loading = "lazy";
    // Hidden originals load only when the editor is opened, avoiding a grid of
    // full-size private images that cannot use the public transformation cache.
    const url = work.deleted_at ? "" : coverUrl(work.cover_url);
    const thumb = work.deleted_at ? "" : thumbUrl(work.cover_url, 320);
    if (thumb) {
      img.src = thumb;
      img.addEventListener("error", () => {
        if (img.src !== url && url) img.src = url;
      });
    } else if (url) {
      img.src = url;
    } else {
      img.src = "data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' width='100' height='100' fill='%23567292'><text x='50%' y='50%' text-anchor='middle' dy='.3em' font-size='12'>无封面</text></svg>";
    }
    thumbWrap.append(img);

    const body = node("div", "admin-card-body");
    const titleRow = node("div", "admin-card-title-row");
    const titleEl = node("strong", "admin-card-title", title);
    titleRow.append(titleEl);
    if (work.deleted_at) titleRow.append(node("span", "status", "已下线"));
    if (work.is_r18) {
      titleRow.append(node("em", "r18-badge", "R18"));
    }

    const meta = node("div", "admin-card-meta");
    meta.append(
      node("span", "admin-card-author", work.author_name || "未署名"),
      node("span", "admin-card-dot", "·"),
      node("span", "admin-card-source", names[work.source] || work.source),
    );

    body.append(titleRow, meta);
    card.append(thumbWrap, body);
    if (state.viewing && state.viewing.id === work.id) card.classList.add("active");
    fragment.append(card);
  }
  $("grid").append(fragment);
}

async function loadWorks(reset = false) {
  if (!reset && (state.loading || state.done)) return;
  if (reset) {
    state.offset = 0;
    state.done = false;
    state.works = [];
    $("grid").replaceChildren();
  }
  state.loading = true;
  const generation = ++state.listGeneration;
  $("list-retry").hidden = true;
  $("list-status").textContent = "正在加载作品…";
  $("list-status").classList.remove("error");
  const query = new URLSearchParams({ limit: String(state.limit), offset: String(state.offset) });
  query.set("status", state.workStatus);
  if (state.source) query.set("source", state.source);
  if (state.q) query.set("q", state.q);
  try {
    const data = await api(`/api/admin/works?${query}`);
    if (generation !== state.listGeneration) return;
    const works = data.works || [];
    state.works.push(...works);
    renderList(works, !reset);
    state.offset += works.length;
    state.done = works.length < state.limit;
    $("list-status").textContent = state.works.length ? `已载入 ${state.works.length} 件${state.workStatus === "hidden" ? "下线作品" : "在线作品"}` : state.workStatus === "hidden" ? "没有已下线作品" : "没有在线作品";
    if ($("nav-works-count")) {
      $("nav-works-count").textContent = state.works.length ? String(state.works.length) : "";
    }
    $("more").hidden = state.done;
  } catch (error) {
    if (error.status === 401 || generation !== state.listGeneration) return;
    $("list-status").textContent = error.message;
    $("list-status").classList.add("error");
    $("list-retry").hidden = false;
  } finally {
    if (generation === state.listGeneration) state.loading = false;
  }
}

async function loadSources() {
  try {
    const data = await api("/api/sources");
    const sources = data.sources || [];

    // 更新下拉选择
    const select = $("source");
    if (select) {
      select.replaceChildren(new Option("全部来源", ""));
      for (const row of sources) {
        select.append(new Option(names[row.source] || row.source, row.source));
      }
      select.value = state.source;
    }

    // 更新侧边栏来源按钮组
    const nav = $("admin-sources");
    if (nav) {
      nav.replaceChildren();
      const allBtn = node("button", state.source === "" ? "source-link active" : "source-link");
      allBtn.type = "button";
      allBtn.dataset.source = "";
      allBtn.append(
        node("span", "source-symbol", "▦"),
        node("span", "", "全部作品"),
      );
      allBtn.addEventListener("click", () => selectSource(""));
      nav.append(allBtn);

      for (const row of sources) {
        const btn = node("button", state.source === row.source ? "source-link active" : "source-link");
        btn.type = "button";
        btn.dataset.source = row.source;
        btn.append(
          node("span", "source-symbol", "•"),
          node("span", "", names[row.source] || row.source),
        );
        if (row.count !== undefined) {
          btn.append(node("span", "source-count", String(row.count)));
        }
        btn.addEventListener("click", () => selectSource(row.source));
        nav.append(btn);
      }
    }
  } catch {
    /* public endpoint; ignore if empty */
  }
}

function selectSource(sourceVal) {
  if (!closeEditor()) {
    $("source").value = state.source;
    return;
  }
  state.source = sourceVal;
  if ($("source")) $("source").value = sourceVal;
  const nav = $("admin-sources");
  if (nav) {
    nav.querySelectorAll(".source-link").forEach((btn) => {
      btn.classList.toggle("active", btn.dataset.source === sourceVal);
    });
  }
  loadWorks(true);
}

function editorPayload() {
  return { title: $("edit-title").value, author_name: $("edit-author").value,
    author_url: $("edit-author-url").value, tags: parseTags($("edit-tags").value), is_r18: $("edit-r18").checked };
}

function hasUnsavedEdit() {
  return Boolean(state.viewing && state.savedEdit && JSON.stringify(editorPayload()) !== state.savedEdit);
}

function canLeaveEditor() {
  if (state.workBusy) {
    $("edit-status").textContent = "操作正在进行，请等待结果。";
    return false;
  }
  return !hasUnsavedEdit() || confirm("有尚未保存的修改，确定丢弃并离开当前作品？");
}

function updateEditorControls() {
  const hidden = Boolean(state.viewing?.deleted_at);
  const busy = state.workBusy || state.editorLoading;
  $("edit-form").querySelectorAll("input, button[type=submit]").forEach((input) => { input.disabled = busy || hidden; });
  updateEditState();
  $("hide-work").hidden = hidden;
  $("hide-work").disabled = busy;
  $("restore-work").hidden = !hidden;
  $("restore-work").disabled = busy || !state.viewing?.can_restore;
  $("image-strip").querySelectorAll("button").forEach((button) => { button.disabled = busy || hidden; });
}

function updateEditState() {
  const hidden = Boolean(state.viewing?.deleted_at);
  const dirty = hasUnsavedEdit();
  const label = $("edit-state");
  if (label) {
    label.textContent = state.workBusy ? "正在处理…" : state.editorLoading ? "正在读取…" : hidden ? "已下线 · 只读" : dirty ? "未保存修改" : "已保存";
    label.classList.toggle("unsaved", dirty);
  }
  const visibility = $("editor-work-state");
  if (visibility) {
    visibility.textContent = hidden ? "已下线" : "在线";
    visibility.classList.toggle("is-hidden", hidden);
  }
}

function fillEditor(work) {
  $("editor-empty").hidden = true;
  $("editor-content").hidden = false;
  $("editor-title").textContent = work.title || work.source_id;
  $("edit-title").value = work.title || "";
  $("edit-author").value = work.author_name || "";
  $("edit-author-url").value = work.author_url || "";
  $("edit-tags").value = (work.tags || []).join(", ");
  $("edit-r18").checked = Boolean(work.is_r18);
  state.savedEdit = JSON.stringify(editorPayload());
  $("edit-status").textContent = work.deleted_at
    ? work.can_restore ? "已下线；元数据和当前图片保留，可恢复公开。" : "图片已被清理，无法在这里恢复。"
    : "";
  updateEditorControls();
}

function closeEditor(force = false) {
  if (force !== true && !canLeaveEditor()) return false;
  state.viewing = null;
  state.savedEdit = "";
  state.editorLoading = false;
  $("editor-empty").hidden = false;
  $("editor-content").hidden = true;
  document.querySelectorAll(".admin-card").forEach((card) => card.classList.remove("active"));
  return true;
}

async function openEditor(work, retry = false) {
  if (!retry && state.viewing?.id === work.id) return;
  if (!canLeaveEditor()) return;
  state.viewing = work;
  state.editorLoading = true;
  state.decisions = {};
  fillEditor(work);
  document.querySelectorAll(".admin-card").forEach((card) => {
    card.classList.toggle("active", card.dataset.id === work.id);
  });
  if (window.matchMedia("(max-width: 980px)").matches) {
    $("editor").scrollIntoView({ behavior: "smooth", block: "start" });
    $("editor").focus({ preventScroll: true });
  }
  $("image-strip").replaceChildren(node("p", "status", "正在加载图片…"));
  try {
    const data = await api(workApiPath(work.id));
    if (state.viewing !== work) return;
    Object.assign(work, data.work);
    fillEditor(work);
    state.images = data.images || [];
    renderImages();
  } catch (error) {
    if (error.status === 401 || state.viewing !== work) return;
    const retryButton = node("button", "text-button", "重试读取作品");
    retryButton.type = "button";
    retryButton.addEventListener("click", () => openEditor(work, true));
    $("image-strip").replaceChildren(node("p", "status error", error.message), retryButton);
  } finally {
    if (state.viewing === work) {
      state.editorLoading = false;
      updateEditorControls();
    }
  }
}

function renderImages() {
  const strip = $("image-strip");
  strip.replaceChildren();
  if (!state.images.length) {
    strip.append(node("p", "status", "没有可显示的图片。"));
    return;
  }
  for (const image of state.images) {
    const figure = node("figure", "admin-image glass-inset");
    const thumbWrap = node("div", "admin-image-thumb");
    const img = node("img");
    img.alt = `第 ${image.page_index + 1} 张`;
    img.src = mediaUrl(image.r2_key);
    thumbWrap.append(img);

    const meta = node("div", "admin-image-meta");
    meta.append(node("span", "admin-image-page", `#${image.page_index + 1}`));

    const actions = node("div", "admin-image-actions");
    if (state.viewing?.deleted_at) {
      actions.append(node("span", "status", "作品已下线"));
    } else if (image.removed) {
      const restore = node("button", "text-button admin-action-restore", "恢复");
      restore.type = "button";
      restore.addEventListener("click", () => restoreImage(image));
      actions.append(restore);
    } else {
      const remove = node("button", "text-button admin-action-delete", "删除");
      remove.type = "button";
      remove.addEventListener("click", () => deleteImage(image));
      actions.append(remove);
    }
    figure.append(thumbWrap, meta, actions);
    strip.append(figure);
  }
  updateEditorControls();
}

async function reviewImage(image, action) {
  const decision_id = state.decisions[image.r2_key] || crypto.randomUUID();
  const payload = {
    decision_id,
    r2_key: image.r2_key,
    action,
    allow_missing_telegram: true,
  };
  if (action === "restore" && image.targets?.length) {
    payload.restored = image.targets.map((target) => ({
      publication_id: target.publication_id,
      message_id: target.message_id,
    }));
  }
  const data = await api("/api/catalog/image-review", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(payload),
  });
  state.decisions[image.r2_key] = decision_id;
  if (data.targets) image.targets = data.targets;
  return data;
}

async function deleteImage(image) {
  if (!state.viewing || state.workBusy || state.editorLoading) return;
  if (!confirm("确定删除这张图片？可在本会话中恢复。")) return;
  state.workBusy = true;
  updateEditorControls();
  $("edit-status").textContent = "正在删除图片…";
  try {
    state.decisions[image.r2_key] = crypto.randomUUID();
    await reviewImage(image, "prepare");
    await reviewImage(image, "delete");
    image.removed = true;
    renderImages();
    $("edit-status").textContent = "已删除图片。如需撤销可点击恢复。";
  } catch (error) {
    if (error.status === 401) return;
    $("edit-status").textContent = error.message;
  } finally {
    state.workBusy = false;
    updateEditorControls();
  }
}

async function restoreImage(image) {
  if (!state.viewing || state.workBusy || state.editorLoading) return;
  state.workBusy = true;
  updateEditorControls();
  $("edit-status").textContent = "正在恢复图片…";
  try {
    await reviewImage(image, "restore");
    delete state.decisions[image.r2_key];
    image.targets = undefined;
    image.removed = false;
    renderImages();
    $("edit-status").textContent = "已成功恢复图片。";
  } catch (error) {
    if (error.status === 401) return;
    $("edit-status").textContent = error.message;
  } finally {
    state.workBusy = false;
    updateEditorControls();
  }
}

/* =====================================================================
   SPA ROUTING: /admin 与 /admin/similar
   ===================================================================== */
function setView(view, updateHistory = true) {
  if (updateHistory) {
    const targetPath = view === "similar" ? "/admin/similar" : "/admin";
    navigate(targetPath);
    return;
  }

  state.view = view;
  const isSimilar = view === "similar";

  // 导航项高亮与状态
  const navWorks = $("nav-works");
  if (navWorks) {
    navWorks.classList.toggle("active", !isSimilar);
    navWorks.setAttribute("aria-current", !isSimilar ? "page" : "false");
  }
  const navSimilar = $("nav-similar");
  if (navSimilar) {
    navSimilar.classList.toggle("active", isSimilar);
    navSimilar.setAttribute("aria-current", isSimilar ? "page" : "false");
  }

  const mobWorks = $("mobile-nav-works");
  if (mobWorks) mobWorks.classList.toggle("active", !isSimilar);
  const mobSimilar = $("mobile-nav-similar");
  if (mobSimilar) mobSimilar.classList.toggle("active", isSimilar);

  // 页面标题与面包屑
  document.title = isSimilar ? "Vitrine · 相似图审批" : "Vitrine · 作品管理";
  const crumb = $("current-view-title");
  if (crumb) {
    crumb.textContent = isSimilar ? "相似图审批" : "作品管理";
  }

  // 视图显隐
  const worksSec = $("view-works-section");
  if (worksSec) worksSec.hidden = isSimilar;
  const simSec = $("similar");
  if (simSec) simSec.hidden = !isSimilar;

  // 侧边栏来源筛选（仅在作品管理显示）
  const sourcesSec = $("sidebar-sources-section");
  if (sourcesSec) sourcesSec.hidden = isSimilar;

  // 顶栏搜索栏显隐
  const searchForm = $("search-form");
  if (searchForm) searchForm.hidden = isSimilar;

  if (isSimilar) {
    loadSimilar();
  } else {
    if (state.works.length === 0 && !state.loading) {
      loadWorks(true);
    }
  }
}

function syncRoute() {
  const path = window.location.pathname;
  const isSimilar = path === "/admin/similar" || path.startsWith("/admin/similar");
  setView(isSimilar ? "similar" : "works", false);
}

function navigate(path, replace = false) {
  if (window.location.pathname !== path && !closeEditor()) return;
  if (window.location.pathname !== path) {
    if (replace) {
      history.replaceState(null, "", path);
    } else {
      history.pushState(null, "", path);
    }
  }
  syncRoute();
}

window.addEventListener("popstate", syncRoute);
window.addEventListener("beforeunload", (event) => {
  if (!hasUnsavedEdit() && !state.workBusy) return;
  event.preventDefault();
  event.returnValue = "";
});

/* =====================================================================
   相似图审批相关渲染与操作
   ===================================================================== */
function groupChecked(box) {
  return [...box.querySelectorAll(".similar-remove:checked")].map((input) => input.value);
}

function renderSimilar(groups, preserveSelection = false) {
  const root = $("similar-groups");
  const selections = new Map(preserveSelection ? [...root.querySelectorAll(".similar-group")].map((box) => [
    box.dataset.id, new Map([...box.querySelectorAll(".similar-remove")].map((input) => [input.value, input.checked])),
  ]) : []);
  root.replaceChildren();

  if (!groups.length) {
    const empty = node("div", "similar-empty-state");
    empty.append(
      node("span", "empty-mark", "✓"),
      node("h2", "", "无待审批相似图"),
      node("p", "", "图库目前没有检测到需要人工审批的疑似重复图片。"),
    );
    root.append(empty);
    return;
  }

  for (const [index, group] of groups.entries()) {
    const box = node("article", "similar-group");
    box.dataset.id = group.id;

    // 找出最高分辨率项
    let bestPixel = 0;
    let bestItem = null;
    for (const item of group.items) {
      const px = (item.width || 0) * (item.height || 0);
      if (px > bestPixel || (px === bestPixel && (item.bytes || 0) > (bestItem?.bytes || 0))) {
        bestPixel = px;
        bestItem = item;
      }
    }

    const head = node("div", "similar-group-head");
    const headLeft = node("div", "");
    headLeft.append(
      node("p", "similar-group-number", `相似组 ${index + 1}`),
      node("h3", "similar-group-title", `${group.items.length} 张图片 · 至少保留一张`),
    );
    head.append(headLeft);
    box.append(head);

    const row = node("div", "similar-row");
    for (const item of group.items) {
      const isInitialKeep = !(selections.get(group.id)?.get(item.r2_key) ?? false);
      const card = node("label", isInitialKeep ? "similar-card glass-inset keep" : "similar-card glass-inset to-delete");

      const thumb = node("div", "similar-thumb");
      const img = node("img");
      img.alt = item.title || item.r2_key;
      img.loading = "lazy";
      img.src = mediaUrl(item.r2_key);
      thumb.append(img);
      thumb.append(node("span", "similar-source", item.source === "x" ? "X" : names[item.source] || "未知来源"));

      const zoomLink = node("a", "similar-zoom-link");
      zoomLink.href = mediaUrl(item.r2_key);
      zoomLink.target = "_blank";
      zoomLink.rel = "noopener noreferrer";
      zoomLink.title = "在新标签页查看高清大图";
      zoomLink.innerHTML = "<span aria-hidden='true'>↗</span> 原图";
      zoomLink.addEventListener("click", (e) => e.stopPropagation());
      thumb.append(zoomLink);

      const controlRow = node("div", "similar-card-control");
      const check = document.createElement("input");
      check.type = "checkbox";
      check.className = "similar-remove";
      check.value = item.r2_key;
      check.checked = !isInitialKeep;
      check.setAttribute("aria-label", `删除这张图片：${item.title || item.work_id}`);

      const badge = node("span", "similar-card-badge", check.checked ? "待删除" : "保留");

      check.addEventListener("change", () => {
        const willDelete = check.checked;
        card.classList.toggle("keep", !willDelete);
        card.classList.toggle("to-delete", willDelete);
        badge.textContent = willDelete ? "待删除" : "保留";
      });

      controlRow.append(check, badge);

      if (item === bestItem) {
        controlRow.append(node("span", "similar-badge-best", "分辨率最高"));
      }

      const info = node("div", "similar-card-info");
      info.append(node("strong", "similar-card-name", item.title || item.work_id));

      const meta = node(
        "span",
        "similar-meta",
        `${item.width}×${item.height} · ${Math.round(item.bytes / 1024)} KB · ${item.kind || "IMG"}`,
      );
      info.append(meta);

      card.append(thumb, controlRow, info);
      row.append(card);
    }
    box.append(row);

    const actions = node("div", "similar-group-actions");
    const keepBtn = node("button", "primary-button similar-btn-keep", "保留整组（跳过）");
    keepBtn.type = "button";
    keepBtn.addEventListener("click", () => skipGroup(group, box));

    const delBtn = node("button", "danger-button similar-btn-del", "确认删除勾选项");
    delBtn.type = "button";
    delBtn.addEventListener("click", () => deleteGroup(group, box));

    actions.append(keepBtn, delBtn);
    box.append(actions);

    root.append(box);
  }
}

function hideGroup(id, box) {
  box.remove();
  state.similar = state.similar.filter((group) => group.id !== id);
  if ($("nav-similar-count")) {
    $("nav-similar-count").textContent = state.similar.length ? String(state.similar.length) : "";
  }
}

function skipBody(group) {
  return {
    id: group.id,
    keys: (group.items || []).map((item) => item.r2_key).filter(Boolean),
  };
}

async function skipGroup(group, box) {
  hideGroup(group.id, box);
  $("similar-status").textContent = `已保留该组。还剩 ${state.similar.length} 组。`;
  try {
    await api("/api/admin/similar/skip", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(skipBody(group)),
    });
  } catch (error) {
    if (error.status === 401) return;
    $("similar-status").textContent = `保留失败，刷新后这组可能恢复：${error.message}`;
  }
}

async function deleteGroup(group, box) {
  const keys = groupChecked(box);
  const keepers = (group.items || []).map((item) => item.r2_key).filter((key) => !keys.includes(key));
  const items = similarDeleteItems(keys, () => keepers);
  if (!items.length) {
    $("similar-status").textContent = "这组没有勾选要删除的图片。";
    return;
  }
  if (!keepers.length) {
    $("similar-status").textContent = "请至少保留这组中的一张图片。";
    return;
  }
  if (!confirm(`确定删除这组中勾选的 ${items.length} 张图片？对应作品图片将下线并清理。`)) return;
  await deleteSimilarItems(items);
}

let similarRefreshTimer;
let similarNeedsRefresh = false;
let similarRequestVersion = 0;
let similarRefreshDelay = 30000;
let similarQuotaBlocked = false;
function scheduleSimilarRefresh(delay = similarRefreshDelay) {
  clearTimeout(similarRefreshTimer);
  similarRefreshTimer = setTimeout(() => {
    if (!similarQuotaBlocked && state.view === "similar" && !document.hidden && !$("console").hidden) loadSimilar(true);
  }, delay);
}
async function loadSimilar(automatic = false) {
  if (automatic && similarQuotaBlocked) return;
  clearTimeout(similarRefreshTimer);
  if (!automatic) {
    similarQuotaBlocked = false;
    similarRefreshDelay = 30000;
  }
  const requestVersion = ++similarRequestVersion;
  const wasWaiting = similarNeedsRefresh;
  similarNeedsRefresh = false;
  $("similar-status").textContent = "正在加载相似组…";
  try {
    const statusP = api("/api/admin/fingerprints/status").then((status) => {
      if (requestVersion !== similarRequestVersion) return status;
      const clipped = status.clipped ?? 0;
      const pendingClip = status.pending_clip ?? Math.max(0, status.online - clipped);
      $("similar-coverage").textContent = clipped
        ? `CLIP ${clipped} / ${status.online}，待扫描 ${pendingClip} · 感知哈希 ${status.hashed}`
        : `已哈希 ${status.hashed} / ${status.online}，待扫描 ${status.pending}（CLIP 尚未完成）`;
      return status;
    });

    const [status, similar] = await Promise.all([
      statusP,
      api("/api/admin/similar"),
    ]);

    if (requestVersion !== similarRequestVersion || state.view !== "similar") return;
    similarRefreshDelay = 30000;
    saveSimilarRetry();
    if (similar.pending) {
      similarNeedsRefresh = true;
      state.similar = [];
      state.autoPairs = [];
      $("similar-groups").replaceChildren();
      $("similar-auto").hidden = true;
      $("similar-clean-dup").hidden = true;
      $("nav-similar-count").textContent = "";
      $("similar-status").textContent = "相似图每天 08:15 更新（北京时间），等待下一次扫描…";
      scheduleSimilarRefresh(300000);
      return;
    }
    state.similar = similar.groups || [];
    state.autoPairs = similar.auto_pairs || [];
    renderSimilar(state.similar, automatic);

    if ($("nav-similar-count")) {
      $("nav-similar-count").textContent = state.similar.length ? String(state.similar.length) : "";
    }

    const autoBtn = $("similar-clean-dup");
    const autoLine = $("similar-auto");
    if (state.autoPairs.length) {
      autoLine.hidden = false;
      autoLine.textContent = `${state.autoPairs.length} 对单图去重建议，确认后保留更高分辨率版本。`;
      autoBtn.hidden = false;
    } else {
      autoLine.hidden = true;
      autoBtn.hidden = true;
    }

    const clipped = status.clipped ?? 0;
    const pendingClip = status.pending_clip ?? Math.max(0, status.online - clipped);
    $("similar-status").textContent = state.similar.length
      ? `已找到 ${state.similar.length} 组疑似重复${similar.engine === "clip" ? "（CLIP 特征聚类）" : "（感知哈希）"}`
      : pendingClip
        ? "暂无需要审批的相似组。CLIP 尚未扫描完成的图片稍后复查。"
        : "没有检测到需要人工审批的相似组。";
    const scanTime = new Date(similar.computed_at || "");
    const scanLabel = Number.isFinite(scanTime.getTime())
      ? `最近扫描：${scanTime.toLocaleString("zh-CN", { timeZone: "Asia/Shanghai", hour12: false })}（北京时间）`
      : "最近扫描时间未知";
    $("similar-status").textContent += ` · ${scanLabel}，每天 08:15 更新（北京时间）。${similar.stale ? "目录有更新，新增图片待下轮扫描；现有候选仍可审批。" : ""}`;
    if (similar.stale) {
      similarNeedsRefresh = true;
      similarRefreshDelay = 300000;
      scheduleSimilarRefresh(300000);
    }
  } catch (error) {
    if (error.status === 401 || requestVersion !== similarRequestVersion || state.view !== "similar") return;
    if (error.code === "database_daily_limit") {
      similarQuotaBlocked = true;
      $("similar-status").textContent = error.message;
      return;
    }
    similarNeedsRefresh = wasWaiting;
    similarRefreshDelay = Math.min(300000, Math.max(similarRefreshDelay * 2, (error.retryAfter || 0) * 1000));
    $("similar-status").textContent = wasWaiting ? "进度暂时无法读取，正在重试…" : error.message;
    if (wasWaiting) scheduleSimilarRefresh();
  }
}

document.addEventListener("visibilitychange", () => {
  if (!document.hidden && !similarQuotaBlocked && similarNeedsRefresh && state.view === "similar" && !$("console").hidden) scheduleSimilarRefresh(300000);
});

/* =====================================================================
   事件监听绑定
   ===================================================================== */
$("login-form")?.addEventListener("submit", async (event) => {
  event.preventDefault();
  $("login-status").textContent = "正在验证管理密钥…";
  try {
    await send("/api/admin/login", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ token: $("login-token").value }),
    });
    $("login-token").value = "";
    showConsole();
    await loadSources();
    syncRoute();
  } catch (error) {
    $("login-status").textContent = error.message === "unauthorized" ? "管理密钥不正确。" : error.message;
  }
});

// 导航项点击
$("nav-works")?.addEventListener("click", (e) => {
  e.preventDefault();
  setView("works");
});
$("nav-similar")?.addEventListener("click", (e) => {
  e.preventDefault();
  setView("similar");
});
$("mobile-nav-works")?.addEventListener("click", (e) => {
  e.preventDefault();
  setView("works");
});
$("mobile-nav-similar")?.addEventListener("click", (e) => {
  e.preventDefault();
  setView("similar");
});

$("editor-close")?.addEventListener("click", () => {
  const card = document.querySelector(".admin-card.active");
  if (closeEditor()) card?.focus();
});
$("similar-refresh")?.addEventListener("click", () => loadSimilar());

$("similar-clean-dup")?.addEventListener("click", async () => {
  const pairs = state.autoPairs.slice(0, 100);
  const items = similarDeleteItems(pairs.map((pair) => pair.remove.r2_key),
    (key) => pairs.filter((pair) => pair.remove.r2_key === key).map((pair) => pair.keep.r2_key));
  if (!items.length) return;
  if (!confirm(`清理 ${items.length} 张单图重复，仅保留更高分辨率版本？`)) return;
  await deleteSimilarItems(items);
});

async function doLogout() {
  if (!canLeaveEditor()) return;
  try {
    await send("/api/admin/logout", { method: "POST" });
  } catch {
    /* ignore error on logout */
  }
  closeEditor(true);
  showLogin();
}

$("logout")?.addEventListener("click", doLogout);
$("topbar-logout")?.addEventListener("click", doLogout);

$("search-form")?.addEventListener("submit", (event) => {
  event.preventDefault();
  if (!closeEditor()) return;
  state.q = $("q").value.trim();
  loadWorks(true);
});

$("source")?.addEventListener("change", () => {
  selectSource($("source").value);
});

$("more")?.addEventListener("click", () => loadWorks());
$("list-retry")?.addEventListener("click", () => loadWorks(state.works.length === 0));
$("work-status")?.addEventListener("change", () => {
  if (!closeEditor()) {
    $("work-status").value = state.workStatus;
    return;
  }
  state.workStatus = $("work-status").value;
  $("grid").setAttribute("aria-label", state.workStatus === "hidden" ? "已下线作品列表" : "在线作品列表");
  loadWorks(true);
});

$("edit-form")?.addEventListener("input", updateEditState);

$("edit-form")?.addEventListener("submit", async (event) => {
  event.preventDefault();
  if (!state.viewing || state.workBusy || state.editorLoading || state.viewing.deleted_at) return;
  const work = state.viewing;
  state.workBusy = true;
  updateEditorControls();
  $("edit-status").textContent = "正在保存更改…";
  try {
    const data = await api(workApiPath(work.id), {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(editorPayload()),
    });
    const patch = {
      title: data.title,
      author_name: data.author_name,
      author_url: data.author_url,
      tags: data.tags,
      is_r18: data.is_r18,
    };
    Object.assign(work, patch);
    Object.assign(state.works.find((item) => item.id === work.id) || {}, patch);
    fillEditor(work);
    renderList(state.works, false);
    $("edit-status").textContent = "修改已保存。";
  } catch (error) {
    if (error.status === 401) return;
    $("edit-status").textContent = error.message;
  } finally {
    state.workBusy = false;
    updateEditorControls();
  }
});

async function changeWorkVisibility(restore) {
  if (!state.viewing || state.workBusy || state.editorLoading) return;
  if (hasUnsavedEdit()) {
    $("edit-status").textContent = "还有未保存的修改，请先保存后再操作作品状态。";
    return;
  }
  const work = state.viewing;
  if (!confirm(restore ? `恢复公开「${work.title || work.id}」？当前保留的图片将重新出现在图库。`
    : `下线「${work.title || work.id}」？公开图库会隐藏作品，元数据和图片仍保留，可从已下线列表恢复。`)) return;
  state.workBusy = true;
  updateEditorControls();
  const action = restore ? "恢复" : "下线";
  const button = restore ? $("restore-work") : $("hide-work");
  $("edit-status").textContent = `正在${action}作品…`;
  try {
    await api(workApiPath(work.id, restore ? "/restore" : "/hide"), { method: "POST" });
    const data = await api(workApiPath(work.id));
    Object.assign(work, data.work);
    fillEditor(work);
    state.images = data.images || [];
    renderImages();
    state.workStatus = work.deleted_at ? "hidden" : "active";
    $("work-status").value = state.workStatus;
    await loadWorks(true);
    $("edit-status").textContent = Boolean(work.deleted_at) === !restore
      ? restore ? "已恢复公开，原有元数据和当前图片已保留。" : "已下线，可在已下线列表找到并恢复。"
      : `作品状态已被其他操作更新，当前${work.deleted_at ? "已下线" : "在线"}，请核实。`;
    button.textContent = restore ? "恢复公开" : "下线作品";
  } catch (error) {
    if (error.status === 401) return;
    const reasons = {
      "work images were removed; cannot restore": "作品图片已被清理，无法恢复。",
      "image originals are missing; cannot restore": "图片原件已被清理，无法恢复。",
      "image originals changed; cannot restore": "图片原件发生变化，已阻止恢复。",
      "work changed; refresh before restoring": "作品已发生变化，请重新打开作品后再试。",
    };
    $("edit-status").textContent = reasons[error.message] || `未确认${action}结果，可安全重试：${error.message}`;
    button.textContent = `重试${action}`;
  } finally {
    state.workBusy = false;
    updateEditorControls();
  }
}

$("hide-work")?.addEventListener("click", () => changeWorkVisibility(false));
$("restore-work")?.addEventListener("click", () => changeWorkVisibility(true));

/* =====================================================================
   初始化
   ===================================================================== */
(async function start() {
  try {
    const session = await send("/api/admin/session");
    if (session.authenticated) {
      showConsole();
      await loadSources();
      syncRoute();
      return;
    }
  } catch {
    /* fall through to login */
  }
  showLogin();
})();

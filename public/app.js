const state = {
  offset: 0,
  limit: 40,
  loading: false,
  done: false,
  works: [],
};

const el = {
  grid: document.getElementById("grid"),
  status: document.getElementById("status"),
  source: document.getElementById("source"),
  tag: document.getElementById("tag"),
  taglist: document.getElementById("taglist"),
  q: document.getElementById("q"),
  apply: document.getElementById("apply"),
  reset: document.getElementById("reset"),
  more: document.getElementById("more"),
  lightbox: document.getElementById("lightbox"),
  lbImg: document.getElementById("lb-img"),
  lbMeta: document.getElementById("lb-meta"),
};

function params() {
  const p = new URLSearchParams();
  p.set("limit", String(state.limit));
  p.set("offset", String(state.offset));
  if (el.source.value) p.set("source", el.source.value);
  if (el.tag.value.trim()) p.set("tag", el.tag.value.trim());
  if (el.q.value.trim()) p.set("q", el.q.value.trim());
  return p;
}

async function loadFilters() {
  const [sources, tags] = await Promise.all([
    fetch("/api/sources").then((r) => r.json()),
    fetch("/api/tags").then((r) => r.json()),
  ]);
  el.source.innerHTML = `<option value="">全部来源</option>`;
  for (const s of sources.sources || []) {
    const opt = document.createElement("option");
    opt.value = s.source;
    opt.textContent = `${s.source} (${s.cnt})`;
    el.source.appendChild(opt);
  }
  el.taglist.innerHTML = "";
  for (const t of tags.tags || []) {
    const opt = document.createElement("option");
    opt.value = t.name;
    el.taglist.appendChild(opt);
  }
}

function renderCards(items, append) {
  if (!append) el.grid.innerHTML = "";
  for (const work of items) {
    const card = document.createElement("article");
    card.className = "card";
    const chips = (work.tags || [])
      .slice(0, 6)
      .map((t) => `<span class="chip">#${escapeHtml(t)}</span>`)
      .join("");
    card.innerHTML = `
      <img src="${work.cover_url || ""}" alt="" loading="lazy" />
      <div class="body">
        <h2 class="title">${escapeHtml(work.title || work.source_id)}</h2>
        <div class="meta">${escapeHtml(work.source)} · ${escapeHtml(work.author_name || "unknown")}${work.is_r18 ? " · R18" : ""} · ${work.page_count}p</div>
        <div class="chips">${chips}</div>
      </div>
    `;
    card.addEventListener("click", () => openLightbox(work));
    el.grid.appendChild(card);
  }
}

function openLightbox(work) {
  el.lbImg.src = work.cover_url || "";
  el.lbMeta.innerHTML = `
    <strong>${escapeHtml(work.title || work.source_id)}</strong><br/>
    <span class="meta">${escapeHtml(work.source)} / ${escapeHtml(work.author_name || "")}</span><br/>
    ${(work.tags || []).map((t) => `<span class="chip">#${escapeHtml(t)}</span>`).join(" ")}
    ${work.source_url ? `<div style="margin-top:8px"><a href="${escapeAttr(work.source_url)}" target="_blank" rel="noopener">原帖链接</a></div>` : ""}
  `;
  el.lightbox.showModal();
}

function escapeHtml(s) {
  return String(s)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}
function escapeAttr(s) {
  return escapeHtml(s).replaceAll("'", "&#39;");
}

async function loadWorks(reset) {
  if (state.loading) return;
  if (reset) {
    state.offset = 0;
    state.done = false;
    state.works = [];
  }
  if (state.done) return;
  state.loading = true;
  el.status.textContent = "加载中…";
  try {
    const data = await fetch(`/api/works?${params()}`).then((r) => r.json());
    const works = data.works || [];
    state.works = reset ? works : state.works.concat(works);
    renderCards(works, !reset);
    state.offset += works.length;
    state.done = works.length < state.limit;
    el.more.hidden = state.done;
    el.status.textContent = `共展示 ${state.works.length} 条${state.done ? "" : "（可继续加载）"}`;
    if (state.works.length === 0) el.status.textContent = "暂无作品。从 hanabi 审批「发送并入库」后会出现在这里。";
  } catch (e) {
    el.status.textContent = `加载失败：${e}`;
  } finally {
    state.loading = false;
  }
}

el.apply.addEventListener("click", () => loadWorks(true));
el.reset.addEventListener("click", () => {
  el.source.value = "";
  el.tag.value = "";
  el.q.value = "";
  loadWorks(true);
});
el.more.addEventListener("click", () => loadWorks(false));
["tag", "q"].forEach((id) => {
  el[id].addEventListener("keydown", (ev) => {
    if (ev.key === "Enter") loadWorks(true);
  });
});

loadFilters().then(() => loadWorks(true));

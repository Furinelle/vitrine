// URL handling is a trust boundary: catalog metadata must never create executable links.
// Run with: node tools/test_frontend.mjs
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const source = readFileSync(new URL('../public/app.js', import.meta.url), 'utf8');
const context = vm.createContext({ URL, URLSearchParams, location: { origin: 'https://gallery.example' } });
vm.runInContext(source.slice(source.indexOf('function safeLink('), source.indexOf('async function getJson(')), context);
for (const value of ['javascript:alert(1)', 'data:text/html,test', '//evil.example', '', null]) {
  assert.equal(context.safeLink(value), '');
}
assert.equal(context.safeLink('https://www.pixiv.net/artworks/123'), 'https://www.pixiv.net/artworks/123');
assert.equal(context.coverUrl('https://evil.example/media/a.png'), '');
assert.equal(context.coverUrl('/media/pixiv/123/a.png'), 'https://gallery.example/media/pixiv/123/a.png');
assert.equal(context.thumbUrl('https://evil.example/media/a.png'), '');
assert.equal(context.thumbUrl('/media/pixiv/123/a.png'), 'https://gallery.example/media/pixiv/123/a.png?width=640');
assert.equal(context.thumbUrl('/media/pixiv/123/a.png', 160), 'https://gallery.example/media/pixiv/123/a.png?width=160');
assert.equal(context.mediaUrl('pixiv/123/a #?.png'), '/media/pixiv/123/a%20%23%3F.png');
console.log('PASS: only HTTP source links and same-origin media, escaped object keys');

// Filtering from a dialog removes its opener; focus must land on stable content.
const events = [];
const dialog = { open: true, close() { this.open = false; events.push('close'); } };
let activateTag;
context.$ = (id) => ({ lightbox: dialog, collection: { focus() { events.push('focus'); } } })[id];
context.button = (_className, _text, action) => { activateTag = action; return { dataset: {} }; };
context.dismissLightbox = async () => dialog.close();
context.applyFilters = ({ tag }) => { assert.equal(tag, 'art'); events.push('filter'); };
context.state = { viewerRequest: null };
context.tagName = (tag) => tag;
context.tagInfo = new Map();
vm.runInContext(source.slice(source.indexOf('function tagButton('), source.indexOf('function coverRatio(')), context);
context.tagButton('art');
await activateTag();
assert.deepEqual(events, ['close', 'filter', 'focus']);
events.length = 0;
await activateTag();
assert.deepEqual(events, ['filter']);
console.log('PASS: dialog tag filtering restores stable focus; sidebar tag filtering leaves focus alone');

// Filter metadata renders independently while preserving unsubmitted search/tag text.
function filtersFixture() {
  const elements = new Map(), pending = new Map();
  function element(id) {
    if (!elements.has(id)) elements.set(id, {
      children: [], dataset: {}, value: '', open: false,
      classList: { toggle() {} }, setAttribute() {}, removeAttribute() {}, addEventListener() {},
      append(...items) { this.children.push(...items); },
      replaceChildren(...items) { this.children = items; },
    });
    return elements.get(id);
  }
  const state = { sources: [], tags: [], works: [], months: [], source: '', tag: '', q: '' };
  const filters = vm.createContext({
    state, names: { x: 'X' }, $: element,
    document: { createElement: () => element(Symbol()), querySelectorAll: () => [], querySelector: () => null },
    Option: class { constructor(text, value) { this.text = text; this.value = value; } },
    applyFilters() {}, tagButton: name => ({ dataset: { tag: name } }), tagInfo: new Map(), tagName: name => name,
    getJson(url) { return new Promise((resolve, reject) => pending.set(url, { resolve, reject })); },
  });
  for (const [start, end] of [
    ['function node(', 'function safeLink('],
    ['function syncFilters(', 'function applyFilters('],
    ['async function loadFilters(', 'function tagWork('],
  ]) vm.runInContext(source.slice(source.indexOf(start), source.indexOf(end)), filters);
  element('q').value = '未提交搜索';
  element('tag').value = '未提交标签';
  element('tag-browser').hidden = true;
  return { filters, state, element, pending };
}
{
  const f = filtersFixture(), loading = f.filters.loadFilters();
  assert.deepEqual([...f.pending.keys()], ['/api/sources', '/api/tags?limit=1000'], 'both metadata requests start together');
  f.state.source = 'x';
  f.pending.get('/api/sources').resolve({ sources: [{ source: 'x', cnt: 12 }] });
  await Promise.resolve();
  assert.equal(f.element('sources').children.length, 3, 'slow tags must not block source navigation (two tabs and the sliding rule)');
  assert.equal(f.element('collection-count').textContent, '12 件收藏', 'source counts render before tags settle');
  assert.equal(f.element('source').value, 'x', 'late metadata uses the current filter');
  assert.equal(f.element('popular-tags').children.length, 0);
  assert.equal(f.element('q').value, '未提交搜索');
  assert.equal(f.element('tag').value, '未提交标签');
  f.pending.get('/api/tags?limit=1000').resolve({ tags: [{ name: 'art', cnt: 4 }] });
  await loading;
  assert.equal(f.element('popular-tags').children[0].dataset.tag, 'art');
  assert.equal(f.element('q').value, '未提交搜索', 'tag completion also preserves the search draft');
  assert.equal(f.element('tag').value, '未提交标签');
}
{
  const f = filtersFixture(), loading = f.filters.loadFilters();
  f.pending.get('/api/sources').reject(new Error('source service unavailable'));
  f.pending.get('/api/tags?limit=1000').resolve({ tags: [{ name: 'still-available', cnt: 2 }] });
  await loading;
  assert.equal(f.element('taglist').children[0].value, 'still-available');
  assert.equal(f.element('popular-tags').children[0].dataset.tag, 'still-available');
  assert.equal(f.element('q').value, '未提交搜索');
  assert.equal(f.element('tag').value, '未提交标签');
}
console.log('PASS: independent metadata rendering, failure isolation, current filters and preserved drafts');

// The tag index groups by kind, splits characters by their work, and shows English names outside Chinese browsers.
for (const language of ['zh-CN', 'en-US']) {
  const elements = new Map();
  const element = (id) => {
    if (!elements.has(id)) elements.set(id, {
      children: [], className: '', textContent: '', value: '', hidden: false, dataset: {},
      addEventListener() {}, setAttribute() {}, style: { setProperty() {} },
      classList: { toggle() {} }, append(...items) { this.children.push(...items); },
      replaceChildren(...items) { this.children = items; },
    });
    return elements.get(id);
  };
  const tags = [
    { name: '原神', cnt: 9, kind: 'work', en: 'GenshinImpact' },
    { name: '芙宁娜', cnt: 5, kind: 'character', parent: '原神', en: 'Furina' },
    { name: '芙卡洛斯', cnt: 4, kind: 'character', parent: '芙宁娜' },
    { name: '初音未来', cnt: 3, kind: 'character', parent: 'VOCALOID', ja: '初音ミク', en: 'Miku' },
    { name: '百合', cnt: 2, kind: 'theme' },
    { name: 'mystery', cnt: 1 },
  ];
  const ctx = vm.createContext({
    navigator: { languages: [language] }, $: element, state: { tags, tagError: '', tag: '芙宁娜' },
    document: { createElement: (tag) => Object.assign(element(Symbol()), { tagName: tag }) },
    tagButton: (name) => ({ name, dataset: { tag: name }, children: [], append(...items) { this.children.push(...items); }, setAttribute() {},
      classList: { add(c) { this.tier = c; }, toggle(_c, on) { this.active = on; } } }),
  });
  vm.runInContext(source.slice(source.indexOf('// 中文浏览器'), source.indexOf('function locationFilters(')) + 'for (const t of state.tags) tagInfo.set(t.name, t);', ctx);
  vm.runInContext(source.slice(source.indexOf('function node('), source.indexOf('function safeLink(')), ctx);
  vm.runInContext(source.slice(source.indexOf('function tagWork('), source.indexOf('function tagButton(')), ctx);
  ctx.renderTagBrowser();
  const groups = element('all-tags').children;
  const titles = groups.map((g) => g.children[0].children[0].textContent);
  assert.deepEqual(groups.map((g) => g.children[0].children[1].textContent), ['1', '3', '1', '1'], 'each kind shows how many tags it has');
  assert.deepEqual(element('tag-kinds').children.map((b) => b.children[0].textContent), titles, 'one jump link per shown kind');
  assert.equal(groups[0].children[1].children[0].classList.tier, 'tier-1', 'the most used tag reads largest');
  assert.deepEqual(titles, language === 'zh-CN' ? ['作品', '角色', '题材', '未分类'] : ['Works', 'Characters', 'Themes', 'Other']);
  const characters = groups[1].children.map((c) => c.children[0]?.textContent ?? (c.textContent || c.children.map((chip) => chip.name).join()));
  // 芙卡洛斯 reaches 原神 through 芙宁娜; 初音未来's parent is not a loaded work, so it lands last.
  assert.deepEqual(characters, [titles[1], language === 'zh-CN' ? '原神' : 'GenshinImpact', '芙宁娜,芙卡洛斯', titles[3], '初音未来']);
  assert.equal(groups[1].children[2].children[0].classList.active, true, 'the current tag is marked');
  assert.equal(ctx.tagLabel('芙宁娜,百合'), language === 'zh-CN' ? '芙宁娜 等 2 个标签' : 'Furina 等 2 个标签');
  assert.equal(vm.runInContext("tagName('芙卡洛斯')", ctx), '芙卡洛斯', 'no English name falls back to the stored name');
  element('tag-search').value = 'miku';
  ctx.renderTagBrowser();
  assert.deepEqual(element('all-tags').children.map((g) => g.children[1].children[0].name), ['初音未来'], 'search matches display names');
}
console.log('PASS: tag index groups by kind and work, follows the browser language, searches every display name');

// The hall: pages of fifteen for the current filter, a numbered pager with gaps, and chapters by game.
{
  const element = (id) => ({ id, children: [], dataset: {}, style: { setProperty(k, val) { this[k] = val; } }, attributes: {}, disabled: false, hidden: false,
    textContent: '', className: '', classList: { add() {}, toggle() {} }, setAttribute(k, val) { this.attributes[k] = val; }, addEventListener() {},
    append(...items) { this.children.push(...items); }, replaceChildren(...items) { this.children = items; } });
  const els = new Map();
  const $ = (id) => { if (!els.has(id)) els.set(id, element(id)); return els.get(id); };
  const filtered = [];
  const hall = vm.createContext({
    $, URLSearchParams, zh: true, names: {}, location: { origin: 'https://gallery.example' },
    state: { source: '', tag: '原神', q: '', p: 1, total: 31, tags: [] },
    document: { createElement: (tag) => Object.assign(element(), { tagName: tag }) },
    middleObserver: null, stage: { cards: new Set() }, galleryUrl: (id) => `/?work=${id}`,
    applyFilters: (changes) => filtered.push(changes), tagName: (name) => name, tagInfo: new Map(),
  });
  vm.runInContext(source.slice(source.indexOf('function node('), source.indexOf('function safeLink(')), hall);
  vm.runInContext('const PAGE_SIZE = 15;' + source.slice(source.indexOf('const pageCount ='), source.indexOf('function renderLoadingHall(')), hall);
  vm.runInContext(source.slice(source.indexOf('// The page one step away'), source.indexOf('let hallTicket')), hall);
  vm.runInContext(source.slice(source.indexOf('// Chapters:'), source.indexOf('\n', source.indexOf('$("pager").hidden')) + 2), hall);
  vm.runInContext(source.slice(source.indexOf('function tagWork('), source.indexOf('function renderTagBrowser(')), hall);
  assert.equal(vm.runInContext('pageCount()', hall), 3, '31 works make three pages of fifteen');
  assert.equal(vm.runInContext("filterQuery({ limit: 15 }).toString()", hall), 'limit=15&tag=%E5%8E%9F%E7%A5%9E');
  assert.deepEqual([vm.runInContext('neighbour(-1)', hall), vm.runInContext('neighbour(1)', hall)], [null, 2]);
  vm.runInContext('state.p = 3; syncPager()', hall);
  assert.equal($('next-page').disabled, true);
  assert.equal($('prev-page').disabled, false);
  vm.runInContext('state.total = 15 * 20; state.p = 10; syncPager()', hall);
  const labels = $('page-list').children.map((li) => li.className === 'page-gap' ? '…' : li.children[0].textContent);
  assert.deepEqual(labels, ['1', '…', '8', '9', '10', '11', '12', '…', '20']);
  vm.runInContext('state.total = 15; state.p = 1; syncPager()', hall);
  assert.equal($('pager').hidden, true, 'a single page needs no pager');
  // Chapters: All, then games by count with the rest behind "more"; a character opens its game's cast.
  const tags = [
    ...Array.from({ length: 10 }, (_, i) => ({ name: `game${i}`, cnt: 100 - i, kind: 'work' })),
    { name: 'hero', cnt: 50, kind: 'character', parent: 'game1' },
    { name: 'twin', cnt: 40, kind: 'character', parent: 'hero' },
    { name: 'other', cnt: 30, kind: 'character', parent: 'game2' },
  ];
  hall.state.tags = tags;
  for (const tag of tags) hall.tagInfo.set(tag.name, tag);
  hall.state.tag = 'twin';
  hall.renderChapters();
  const chips = $('chapters').children;
  assert.equal(chips[0].textContent, '全部');
  assert.deepEqual(chips.slice(1, 8).map((c) => c.children[0].textContent), ['game0', 'game1', 'game2', 'game3', 'game4', 'game5', 'game6']);
  assert.equal(chips.at(-1).textContent, '更多 3');
  assert.equal(chips[2].className, 'chapter active', 'a character marks its game, through a parent character');
  assert.equal($('cast').hidden, false);
  assert.deepEqual($('cast').children.map((c) => c.children[0].textContent), ['hero', 'twin']);
  assert.equal($('cast').children[1].className, 'chapter active');
  chips[3].action?.();
  hall.state.tag = '';
  hall.renderChapters();
  assert.equal($('chapters').children[0].className, 'chapter active');
  assert.equal($('cast').hidden, true);
  // Spread sizes: one large, four medium, ten small; the large cover asks for the bigger thumbnail.
  vm.runInContext('var artColor = () => "", openLightbox = () => {}, thumbUrl = (u, w) => u + "?width=" + w, coverUrl = (u) => u;', hall);
  vm.runInContext(source.slice(source.indexOf('function hallItem('), source.indexOf('function renderLoadingHall(')), hall);
  const items = Array.from({ length: 15 }, (_, i) => hall.hallItem({ id: `x:${i}`, source: 'x', page_count: 1, cover_url: `/media/x/${i}.jpg` }, i));
  assert.deepEqual([...new Set(items.map((item) => item.className))], ['hall-item lead', 'hall-item medium', 'hall-item small']);
  assert.equal(items.filter((item) => item.className.endsWith('medium')).length, 4);
  const img = (item) => item.children[0].children[0];
  assert.match(img(items[0]).src, /width=1280/);
  assert.match(img(items[1]).src, /width=640/);
  assert.equal(img(items[0]).fetchPriority, 'high');
  assert.equal(img(items[4]).loading, 'eager');
  assert.equal(img(items[5]).loading, 'lazy');
}
console.log('PASS: hall pages of fifteen, numbered pager with gaps, chapters by game with a cast row, lead/medium/small spread');


const adminSource = readFileSync(new URL('../public/admin.js', import.meta.url), 'utf8');
const admin = vm.createContext({});
vm.runInContext(
  adminSource.slice(adminSource.indexOf('function parseTags('), adminSource.indexOf('function node(')),
  admin,
);
assert.equal(admin.parseTags('  #foo, bar，baz  ').join(','), 'foo,bar,baz');
assert.equal(admin.parseTags('foo, foo').join(','), 'foo');
assert.equal(admin.workApiPath('pixiv:123'), '/api/admin/works/pixiv%3A123');
assert.equal(admin.workApiPath('pixiv:123', '/hide'), '/api/admin/works/pixiv%3A123/hide');
console.log('PASS: admin tag parse and work id encoding');

// Admin SPA routing keeps the shared navigation and work filters in sync.
const elementMap = {
  'nav-works': { classList: { toggle(c, f) { if (f) this[c] = true; else delete this[c]; } }, setAttribute(k, v) { this[k] = v; } },
  'nav-similar': { classList: { toggle(c, f) { if (f) this[c] = true; else delete this[c]; } }, setAttribute(k, v) { this[k] = v; } },
  'nav-tags': { classList: { toggle(c, f) { if (f) this[c] = true; else delete this[c]; } }, setAttribute(k, v) { this[k] = v; } },
  'view-works-section': { hidden: false },
  'similar': { hidden: true },
  'tags-view': { hidden: true },
  'sidebar-sources-section': { hidden: false },
  'search-form': { style: {} },
};
const routingContext = vm.createContext({
  window: { location: { pathname: '/admin/similar' }, addEventListener() {} },
  document: { title: '' },
  $: (id) => elementMap[id] || null,
  state: { view: 'works', works: [], similar: [], loading: false },
  loadSimilar() {},
  loadWorks() {},
  loadTagsAdmin() { routingContext.tagsLoaded = true; },
});
vm.runInContext(
  adminSource.slice(adminSource.indexOf('const VIEWS = {'), adminSource.indexOf('/* =====================================================================\n   相似图审批相关渲染与操作')),
  routingContext,
);

routingContext.syncRoute();
assert.equal(routingContext.state.view, 'similar');
assert.equal(routingContext.document.title, 'Vitrine · 相似图审批');
assert.equal(elementMap['view-works-section'].hidden, true);
assert.equal(elementMap['similar'].hidden, false);
assert.equal(elementMap['nav-similar'].classList.active, true);
assert.equal(elementMap['sidebar-sources-section'].hidden, true);
assert.equal(elementMap['search-form'].hidden, true);

routingContext.window.location.pathname = '/admin';
routingContext.syncRoute();
assert.equal(routingContext.state.view, 'works');
assert.equal(routingContext.document.title, 'Vitrine · 作品管理');
assert.equal(elementMap['view-works-section'].hidden, false);
assert.equal(elementMap['similar'].hidden, true);
assert.equal(elementMap['nav-works'].classList.active, true);
assert.equal(elementMap['sidebar-sources-section'].hidden, false);
assert.equal(elementMap['search-form'].hidden, false);

routingContext.window.location.pathname = '/admin/tags';
routingContext.syncRoute();
assert.equal(routingContext.state.view, 'tags');
assert.equal(routingContext.document.title, 'Vitrine · 标签管理');
assert.deepEqual([elementMap['view-works-section'].hidden, elementMap['similar'].hidden, elementMap['tags-view'].hidden], [true, true, false]);
assert.equal(elementMap['nav-tags'].classList.active, true);
assert.equal(elementMap['search-form'].hidden, true);
assert.equal(routingContext.tagsLoaded, true);
console.log('PASS: admin SPA routing /admin, /admin/similar and /admin/tags');

// Shared links preserve filters and image position, and never concatenate raw metadata.
const urls = vm.createContext({ URLSearchParams, state: { source: 'x', tag: 'a & b', q: '<title>' } });
vm.runInContext(source.slice(source.indexOf('function galleryUrl('), source.indexOf('function syncFilters(')), urls);
const shared = new URL(urls.galleryUrl('x:123?evil#', 2), 'https://gallery.example');
assert.equal(shared.searchParams.get('work'), 'x:123?evil#');
assert.equal(shared.searchParams.get('page'), '3');
assert.equal(shared.searchParams.get('tag'), 'a & b');
assert.equal(new URL(urls.galleryUrl(), shared).searchParams.has('work'), false);
assert.equal(context.thumbUrl('/media/a.png', 9000), '');
assert.equal(context.safeLink('https://user:password@example.com'), '');
console.log('PASS: share URL round-trip and bounded thumbnail presets');

// A page turn that resolves late must not overwrite a newer one.
{
  const pending = [], drawn = [];
  const el = () => ({ hidden: false, textContent: '', classList: { add() {}, remove() {} }, setAttribute() {}, replaceChildren() {} });
  const els = new Map();
  const turns = vm.createContext({
    state: { works: [], loading: false }, $: (id) => { if (!els.has(id)) els.set(id, el()); return els.get(id); },
    syncPager() {}, renderFeatured() {}, neighbour: () => null,
    fetchPage: () => new Promise((resolve) => pending.push(resolve)),
    renderHall: async (works) => { drawn.push(works.map((w) => w.id).join()); },
  });
  vm.runInContext(source.slice(source.indexOf('let hallTicket = 0;'), source.indexOf('// The total for the current filters')), turns);
  const first = turns.showPage(1), second = turns.showPage(1);
  pending[1]({ works: [{ id: 'new' }] });
  await second;
  pending[0]({ works: [{ id: 'old' }] });
  await first;
  assert.deepEqual(drawn, ['new']);
  assert.equal(turns.state.works[0].id, 'new');
  assert.equal(turns.state.loading, false);
}
console.log('PASS: a late page cannot overwrite a newer turn');

// dialog.close() queues its close event. A detail response may resolve first,
// so filtering must cancel it synchronously and never rewrite the new URL.
const viewerElements = new Map();
function viewerElement(id) {
  if (!viewerElements.has(id)) viewerElements.set(id, {
    open: false, hidden: false, textContent: '', style: {},
    classList: { add() {}, remove() {}, toggle() {}, contains() { return false; } },
    getBoundingClientRect() { return {x:0,y:0,width:1000,height:700}; },
    setAttribute() {}, removeAttribute() {}, replaceChildren() {},
    querySelectorAll() { return []; },
    showModal() { this.open = true; },
    close() { this.open = false; }, // The queued close listener has not run yet.
  });
  return viewerElements.get(id);
}
const viewerState = { source: '', tag: '', q: '', month: '', p: 1, months: [], viewerRequest: null };
const viewerRoutes = [];
const viewerHistory = {
  state: null,
  pushState(state, _title, url) { this.state = state; viewerRoutes.push(url); },
  replaceState(state, _title, url) { this.state = state; viewerRoutes.push(url); },
};
let completeDetail;
const viewer = vm.createContext({
  URL, URLSearchParams, AbortController,
  location: { origin: 'https://gallery.example' }, history: viewerHistory,
  state: viewerState, $: viewerElement, window: { scrollTo() {} },
  animateLightbox() {}, dismissLightbox() { viewerElement('lightbox').close(); },
  getJson() { return new Promise(resolve => { completeDetail = resolve; }); },
  showWorkInfo() {}, syncFilters() {}, loadHall() {},
});
for (const [start, end] of [
  ['function mediaUrl(', 'async function getJson('],
  ['function galleryUrl(', 'function syncFilters('],
  ['function applyFilters(', 'async function loadFilters('],
  ['function showImage(', 'function showWorkInfo('],
  ['async function openLightbox(', 'function syncRoute('],
]) vm.runInContext(source.slice(source.indexOf(start), source.indexOf(end)), viewer);
const pendingDetail = viewer.openLightbox({ id: 'pixiv:123' }, null);
const detailController = viewerState.viewerRequest;
viewer.applyFilters({ tag: 'new-tag' });
completeDetail({ work: { id: 'pixiv:123' }, images: [{ r2_key: 'pixiv/123/0.png' }] });
await pendingDetail;
assert.equal(viewerElement('lightbox').open, false);
assert.equal(detailController.signal.aborted, true);
assert.equal(viewerState.viewerRequest, null);
assert.deepEqual(viewerRoutes, ['/?work=pixiv%3A123', '/?tag=new-tag']);
assert.equal(viewerRoutes.at(-1).includes('work='), false);
console.log('PASS: closing during a detail request cannot rewrite the new filter URL');


// Shared theme boots before either page script, without their $ helper.
const themeSource = readFileSync(new URL('../public/theme.js', import.meta.url), 'utf8');
for (const saved of [null, 'dark', 'light', 'invalid', 'blocked']) for (const withViewport of [false, true]) {
  const root = { style: {} }, meta = {}, toggle = { label: '', click: null };
  const input = { addEventListener() {} };
  const toggleElement = {
    setAttribute(_name, value) { toggle.label = value; },
    addEventListener(_name, callback) { toggle.click = callback; },
  };
  const boot = vm.createContext({
    window: { scrollY: 0, addEventListener() {}, visualViewport: withViewport ? { addEventListener() {} } : undefined },
    document: { documentElement: root, querySelector: () => meta,
      getElementById: id => id === 'theme-toggle' ? toggleElement : ['q', 'back-to-top'].includes(id) ? input : null },
    localStorage: { getItem() { if (saved === 'blocked') throw new Error('blocked'); return saved; }, setItem() {} },
  });
  vm.runInContext(themeSource, boot);
  assert.equal(root.style.colorScheme, saved === 'dark' ? 'dark' : 'light');
  toggle.click();
  assert.equal(root.style.colorScheme, saved === 'dark' ? 'light' : 'dark');
}
console.log('PASS: shared theme boots independently, honors saved choices, toggles and tolerates blocked storage');

// Floating controls keep draft text while the software keyboard pans the viewport.
const scrollEvents = new Map(), viewportEvents = new Map(), scrollCalls = [];
let focusedOptions, viewportOffset;
const scrollElements = {
  q: { value: '未提交的搜索', addEventListener(type, cb) { scrollEvents.set(`q:${type}`, cb); } },
  'back-to-top': { hidden: true, addEventListener(type, cb) { scrollEvents.set(`top:${type}`, cb); } },
  collection: { focus(options) { focusedOptions = options; } },
};
const scrollWindow = {
  scrollY: 0,
  scrollTo(options) { scrollCalls.push(options); },
  addEventListener(type, cb) { scrollEvents.set(type, cb); },
  visualViewport: { scale: 1, offsetTop: 140, addEventListener(type, cb) { viewportEvents.set(type, cb); } },
};
const scrollDocument = {
  activeElement: scrollElements.q,
  getElementById: id => scrollElements[id],
  querySelector(selector) { return selector === "main" ? scrollElements.collection : { style: { setProperty(_name, value) { viewportOffset = value; } } }; },
};
const controls = vm.createContext({ window: scrollWindow, document: scrollDocument, $: id => scrollElements[id] });
vm.runInContext(themeSource.slice(themeSource.indexOf('function syncSearchViewport(')), controls);
assert.equal(scrollElements['back-to-top'].hidden, true);
scrollWindow.scrollY = 1200;
scrollEvents.get('scroll')();
assert.equal(scrollElements['back-to-top'].hidden, false);
viewportEvents.get('scroll')();
assert.equal(viewportOffset, '140px');
assert.equal(scrollElements.q.value, '未提交的搜索');
scrollWindow.visualViewport.offsetTop = -10;
scrollEvents.get('q:focus')();
assert.equal(viewportOffset, '0px', 'viewport offset cannot move the header above its anchor');
scrollWindow.visualViewport.offsetTop = 140;
scrollWindow.visualViewport.scale = 2;
viewportEvents.get('resize')();
assert.equal(viewportOffset, '0px', 'pinch zoom must retain its native pan');
scrollWindow.visualViewport.scale = 1;
viewportEvents.get('resize')();
assert.equal(viewportOffset, '140px', 'keyboard pan resumes after pinch zoom ends');
scrollDocument.activeElement = null;
scrollEvents.get('q:blur')();
assert.equal(viewportOffset, '0px');
scrollEvents.get('top:click')();
assert.equal(focusedOptions.preventScroll, true);
assert.equal(scrollCalls.at(-1).top, 0);
assert.equal(scrollCalls.at(-1).behavior, 'auto', 'CSS preserves reduced-motion preference');
scrollWindow.scrollY = 0;
scrollEvents.get('scroll')();
assert.equal(scrollElements['back-to-top'].hidden, true);
scrollWindow.visualViewport = undefined;
controls.syncSearchViewport();
assert.equal(viewportOffset, '0px');
console.log('PASS: back-to-top visibility/focus, keyboard viewport offset, draft preservation and zoom fallback');

// A platform error page must produce a readable message, while auth still expires normally.
const requestFailure = vm.createContext({ fetch: async () => ({
  ok: false, status: 500, headers: new Headers(), json: async () => { throw new Error('HTML response'); },
}) });
vm.runInContext(adminSource.slice(adminSource.indexOf('async function send('), adminSource.indexOf('const state =')), requestFailure);
await assert.rejects(requestFailure.send('/api/admin/similar'), /暂时无法加载/);
requestFailure.fetch = async () => ({ ok: false, status: 429, headers: new Headers(), json: async () => { throw new Error('HTML response'); } });
await assert.rejects(requestFailure.send('/api/admin/similar'), /请求太频繁/);
requestFailure.fetch = async () => ({ ok: false, status: 401, headers: new Headers(), json: async () => { throw new Error('HTML response'); } });
await assert.rejects(requestFailure.send('/api/admin/similar'), error => error.status === 401 && error.message === 'unauthorized');
requestFailure.fetch = async () => ({ ok: false, status: 503, headers: new Headers({ 'Retry-After': '3600' }),
  json: async () => ({ ok: false, code: 'database_daily_limit', error: '数据库今日访问额度已用完。' }) });
await assert.rejects(requestFailure.send('/api/admin/similar'), error => error.status === 503 &&
  error.code === 'database_daily_limit' && error.retryAfter === 3600 && error.message === '数据库今日访问额度已用完。');
console.log('PASS: non-JSON errors stay readable, auth expires and daily quota metadata survives');

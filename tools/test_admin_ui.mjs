import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const html = readFileSync(new URL('../public/admin.html', import.meta.url), 'utf8');
const source = readFileSync(new URL('../public/admin.js', import.meta.url), 'utf8');
const css = readFileSync(new URL('../public/styles.css', import.meta.url), 'utf8');
const galleryHtml = readFileSync(new URL('../public/index.html', import.meta.url), 'utf8');
assert.ok(html.indexOf('/theme.css') < html.indexOf('/styles.css'));
assert.ok(html.indexOf('/theme.js') < html.indexOf('/admin.js'));
assert.doesNotMatch(html, /glass\.js/, 'the CSS glass needs no per-frame script');
assert.equal((html.match(/id="theme-toggle"/g) || []).length, 1);
for (const page of [html, galleryHtml]) {
  const ids = [...page.matchAll(/\bid="([^"]+)"/g)].map(match => match[1]);
  assert.equal(new Set(ids).size, ids.length, 'DOM IDs must be unique');
  const headers = [...page.matchAll(/<header class="topbar">([\s\S]*?)<\/header>/g)];
  assert.equal(headers.length, 1, 'one shared floating header');
  assert.match(headers[0][1], /class="brand liquid-glass"/);
  for (const id of ['search-form', 'q', 'theme-toggle']) {
    assert.ok(headers[0][1].includes(`id="${id}"`), id);
  }
  assert.ok(page.indexOf('/theme.css') < page.indexOf('/theme.js'));
  assert.doesNotMatch(page, /glass\.js/);
}
assert.ok(html.indexOf('</header>') < html.indexOf('id="console"'), 'theme remains available during login');
assert.doesNotMatch(html, /id="(?:admin-sidebar|mobile-nav-works|mobile-nav-similar)"/);
for (const script of ['theme', 'admin']) {
  assert.match(html, new RegExp(`<script src="/${script}\\.js[^>]* defer>`));
}
for (const id of ['login', 'login-form', 'login-token', 'console', 'search-form', 'q', 'source',
  'work-status', 'grid', 'editor', 'edit-form', 'hide-work', 'restore-work', 'image-strip',
  'similar-groups', 'similar-refresh', 'similar-clean-dup', 'nav-works', 'nav-similar',
  'nav-works-count', 'nav-similar-count', 'admin-sources', 'sidebar-sources-section', 'logout']) {
  assert.ok(html.includes(`id="${id}"`), id);
}
assert.doesNotMatch(html + css, /FICHE|GROUPE|ATELIER|Plus Jakarta|JetBrains|--font-display/);
assert.match(css, /\.admin-card-thumb img\s*\{[^}]*object-fit: cover/);
assert.match(css, /\.admin-image-thumb img, \.similar-thumb img\s*\{[^}]*object-fit: contain/);
assert.match(css, /\.admin-editor:has\(#editor-content\[hidden\]\)\s*\{[^}]*display: none/);
assert.match(css, /\.admin-split-layout:has\(#editor-content\[hidden\]\)\s*\{\s*grid-template-columns: minmax\(0, 1fr\);/);
console.log('PASS: shared header, unique controls, cover cards, complete review images and collapsed empty editor');

function element(tag = '') {
  return {
    tag, children: [], textContent: '', checked: false, dataset: {}, attributes: {}, listeners: {},
    classList: { toggle(name, value) { this[name] = value; } },
    append(...children) { this.children.push(...children); },
    replaceChildren(...children) { this.children = children; },
    querySelectorAll(selector) {
      return this.children.flatMap(child => [
        ...(child.className?.split(' ').includes(selector.slice(1)) ? [child] : []),
        ...child.querySelectorAll(selector),
      ]);
    },
    setAttribute(key, value) { this.attributes[key] = value; },
    addEventListener(name, callback) { this.listeners[name] = callback; },
  };
}
const elements = new Map(['edit-state', 'editor-work-state', 'similar-groups'].map(id => [id, element()]));
let dirty = false;
const state = { viewing: { deleted_at: null }, workBusy: false, editorLoading: false };
const context = vm.createContext({
  state, $: id => elements.get(id), hasUnsavedEdit: () => dirty,
  document: { createElement: element }, mediaUrl: key => '/media/' + key,
  node(tag, className, text) { const el = element(tag); el.className = className; el.textContent = text || ''; return el; },
});
vm.runInContext(source.slice(source.indexOf('function updateEditState('), source.indexOf('function fillEditor(')), context);
context.updateEditState();
assert.equal(elements.get('edit-state').textContent, '已保存');
dirty = true;
context.updateEditState();
assert.equal(elements.get('edit-state').textContent, '未保存修改');
assert.equal(elements.get('edit-state').classList.unsaved, true);
state.workBusy = true;
context.updateEditState();
assert.equal(elements.get('edit-state').textContent, '正在处理…');
state.workBusy = false;
state.editorLoading = true;
context.updateEditState();
assert.equal(elements.get('edit-state').textContent, '正在读取…');
state.editorLoading = false;
dirty = false;
state.viewing.deleted_at = 'date';
context.updateEditState();
assert.equal(elements.get('edit-state').textContent, '已下线 · 只读');
assert.equal(elements.get('editor-work-state').textContent, '已下线');
assert.equal(elements.get('editor-work-state').classList['is-hidden'], true);
console.log('PASS: separate saved, dirty, busy, loading and hidden indicators');

vm.runInContext(source.slice(source.indexOf('const names ='), source.indexOf('function parseTags(')), context);
vm.runInContext(source.slice(source.indexOf('function renderSimilar('), source.indexOf('function hideGroup(')), context);
const reviewGroups = [{ id: 'local-group', items: [
  { keep: true, r2_key: 'keep', source: 'x', title: '保留图', work_id: 'x:1', width: 200, height: 300, bytes: 5000 },
  { keep: false, r2_key: 'remove', source: 'pixiv', title: '重复图', work_id: 'pixiv:2', width: 100, height: 150, bytes: 2000 },
  { keep: false, r2_key: 'douyin', source: 'douyin', title: '抖音图', work_id: 'douyin:3', width: 100, height: 150, bytes: 2000 },
  { keep: false, r2_key: 'unknown', title: '未知来源图', work_id: 'unknown:4', width: 100, height: 150, bytes: 2000 },
] }];
context.renderSimilar(reviewGroups);
const group = elements.get('similar-groups').children[0];
const row = group.children[1];
assert.deepEqual(row.querySelectorAll('.similar-source').map(badge => badge.textContent), ['X', 'Pixiv', '抖音', '未知来源']);
const keep = row.children[0].children[1];
const remove = row.children[1].children[1];
assert.equal(keep.children[0].checked, false);
assert.equal(keep.children[1].textContent, '保留');
assert.equal(remove.children[0].checked, false);
assert.equal(remove.children[1].textContent, '保留');
assert.ok(row.querySelectorAll('.similar-remove').every(input => !input.checked), 'all deletions start unselected, regardless of recommendations');
assert.equal(remove.children[0].attributes['aria-label'], '删除这张图片：重复图');
remove.children[0].checked = true;
remove.children[0].listeners.change();
assert.equal(remove.children[1].textContent, '待删除');
assert.equal(row.children[1].classList.keep, false);
assert.equal(row.children[1].classList['to-delete'], true);
context.renderSimilar(reviewGroups, true);
assert.equal(elements.get('similar-groups').children[0].children[1].children[1].children[1].children[0].checked, true,
  'automatic refresh preserves user checkbox choices');
context.renderSimilar(reviewGroups);
assert.equal(elements.get('similar-groups').children[0].children[1].children[1].children[1].children[0].checked, false,
  'manual refresh starts a new unselected review');
console.log('PASS: deletion checkbox keeps its original semantics and readable state');

// Cancelling the unsaved-edit prompt must not move focus out of the editor.
let allowClose = false, refocused = 0;
elements.set('editor-close', element());
context.closeEditor = () => allowClose;
context.document.querySelector = () => ({ focus() { refocused++; } });
const closeStart = source.indexOf('$("editor-close")?.addEventListener');
vm.runInContext(source.slice(closeStart, source.indexOf('\n});', closeStart) + 4), context);
elements.get('editor-close').listeners.click();
assert.equal(refocused, 0);
allowClose = true;
elements.get('editor-close').listeners.click();
assert.equal(refocused, 1);
console.log('PASS: closing the editor restores card focus only after confirmation');

// All HTTP errors retain their status, quota code and retry delay, even for non-JSON failures.
let responseBody = { ok: false, code: 'database_daily_limit', error: '数据库今日访问额度已用完，每日北京时间 8 点重置。请稍后手动重试。' };
let responseStatus = 503;
const transport = vm.createContext({
  async fetch() { return { ok: false, status: responseStatus, headers: { get() { return '7200'; } },
    async json() { if (responseBody === null) throw new Error('not JSON'); return responseBody; } }; },
});
vm.runInContext(source.slice(source.indexOf('async function send('), source.indexOf('const state =')), transport);
await assert.rejects(transport.send('/test'), error => error.status === 503 &&
  error.code === 'database_daily_limit' && error.retryAfter === 7200 && /北京时间 8 点/.test(error.message));
responseBody = null;
await assert.rejects(transport.send('/test'), error => error.status === 503 &&
  error.code === undefined && /稍后重试/.test(error.message));
responseStatus = 401;
await assert.rejects(transport.send('/test'), error => error.status === 401 && error.message === 'unauthorized');
responseStatus = 503;
console.log('PASS: request errors preserve quota metadata, non-JSON status and expired-session handling');

// Pending Oracle results must never be presented as an empty successful scan or stale deletion plan.
const pendingElements = new Map(['similar-status','similar-coverage','similar-groups','similar-auto',
  'similar-clean-dup','nav-similar-count','console'].map(id => [id, element()]));
let retryTimer, retryDelay, visibilityChange, rendered = 0, requests = 0;
let similarResponse = {pending:true,groups:[],auto_pairs:[],engine:'hash'};
const offloadState = { view: 'similar', similar: [{id:'old'}], autoPairs: [{id:'old'}] };
const offloadDocument = { hidden: false, addEventListener(name, callback) { visibilityChange = callback; } };
const offload = vm.createContext({
  state: offloadState, $: id => pendingElements.get(id), document: offloadDocument,
  clearTimeout() { retryTimer = null; }, setTimeout(callback, delay) { retryTimer = callback; retryDelay = delay; },
  saveSimilarRetry() {}, renderSimilar() { rendered++; },
  async api(path) { requests++; return path.includes('fingerprints')
    ? {clipped:2,online:4,hashed:4,pending_clip:2}
    : similarResponse; },
});
vm.runInContext(source.slice(source.indexOf('let similarRefreshTimer;'), source.indexOf('/* =====================================================================\n   事件监听绑定')), offload);
await offload.loadSimilar();
assert.equal(rendered, 0);
assert.equal(offloadState.similar.length, 0);
assert.equal(offloadState.autoPairs.length, 0);
assert.equal(pendingElements.get('similar-clean-dup').hidden, true);
assert.match(pendingElements.get('similar-status').textContent, /每天 08:15 更新（北京时间）/);
assert.equal(retryDelay, 300000, 'daily pending results poll every five minutes');
offloadDocument.hidden = true;
const oldRequests = requests;
retryTimer();
assert.equal(requests, oldRequests, 'background pages must not poll');
offloadDocument.hidden = false;
visibilityChange();
assert.equal(retryDelay, 300000, 'returning to pending results keeps the five-minute interval');
const normalApi = offload.api;
offload.api = transport.send;
retryTimer = null;
await offload.loadSimilar(true);
assert.match(pendingElements.get('similar-status').textContent, /重试/);
assert.equal(typeof retryTimer, 'function', 'pending progress resumes after transient errors');
assert.equal(retryDelay, 300000, 'transient HTTP Retry-After is capped at five minutes');
responseBody = { ok: false, code: 'database_daily_limit', error: '数据库今日访问额度已用完，每日北京时间 8 点重置。请稍后手动重试。' };
await offload.loadSimilar(true);
assert.equal(retryTimer, null, 'daily quota errors stop automatic retries');
assert.match(pendingElements.get('similar-status').textContent, /北京时间 8 点/);
visibilityChange();
assert.equal(retryTimer, null, 'returning to the page cannot restart quota polling');
await offload.loadSimilar(true);
assert.equal(retryTimer, null, 'automatic loading stays blocked until manual retry');
offload.api = normalApi;
await offload.loadSimilar();
assert.equal(retryDelay, 300000, 'manual refresh resumes normal pending polling');
offload.api = async () => { throw new Error('temporary network failure'); };
for (const delay of [60000, 120000, 240000, 300000, 300000]) {
  await offload.loadSimilar(true);
  assert.equal(retryDelay, delay, 'repeated errors back off exponentially to five minutes');
}
offload.api = normalApi;
similarResponse = {pending:false,groups:[],auto_pairs:[],engine:'hash'};
await offload.loadSimilar();
assert.equal(rendered, 1);
assert.doesNotMatch(pendingElements.get('similar-status').textContent, /等待下一次扫描/);
assert.match(pendingElements.get('similar-status').textContent, /最近扫描时间未知/);
assert.equal(retryTimer, null, 'current cached results do not poll');
console.log('PASS: daily pending results clear stale proposals, poll every five minutes, back off errors and stop on daily quota');

similarResponse = {pending:false,stale:true,computed_at:'2026-10-10T00:15:00Z',
  groups:[{id:'reviewable'}],auto_pairs:[{id:'auto-reviewable'}],engine:'clip'};
await offload.loadSimilar();
assert.equal(rendered, 2);
assert.equal(offloadState.similar[0].id, 'reviewable');
assert.equal(offloadState.autoPairs[0].id, 'auto-reviewable');
assert.equal(pendingElements.get('similar-clean-dup').hidden, false);
assert.equal(pendingElements.get('nav-similar-count').textContent, '1');
assert.match(pendingElements.get('similar-status').textContent, /已找到 1 组/);
assert.match(pendingElements.get('similar-status').textContent, /2026\/10\/10 08:15:00（北京时间）/);
assert.match(pendingElements.get('similar-status').textContent, /每天 08:15 更新（北京时间）/);
assert.match(pendingElements.get('similar-status').textContent, /新增图片待下轮扫描；现有候选仍可审批/);
assert.equal(retryDelay, 300000, 'stale cached candidates poll every five minutes');
offloadDocument.hidden = true;
const staleRequests = requests;
retryTimer();
assert.equal(requests, staleRequests, 'stale cached candidates do not poll in background');
offloadDocument.hidden = false;
visibilityChange();
assert.equal(retryDelay, 300000, 'returning to stale cached candidates keeps the five-minute interval');
offload.api = async () => { throw new Error('temporary network failure'); };
await offload.loadSimilar(true);
assert.equal(offloadState.similar[0].id, 'reviewable', 'transient polling errors retain reviewable candidates');
assert.equal(retryDelay, 300000, 'stale polling errors cannot increase the request frequency');
offload.api = normalApi;
similarResponse.computed_at = 'invalid';
await offload.loadSimilar(true);
assert.match(pendingElements.get('similar-status').textContent, /最近扫描时间未知/);
similarResponse = {pending:true,groups:[],auto_pairs:[],engine:'clip'};
await offload.loadSimilar(true);
assert.equal(offloadState.similar.length, 0, 'genuinely missing cache still clears candidates');
assert.equal(offloadState.autoPairs.length, 0);
assert.equal(pendingElements.get('similar-clean-dup').hidden, true);
console.log('PASS: stale cache remains reviewable with Beijing scan time, pending update notice and five-minute polling');

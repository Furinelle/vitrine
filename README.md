# Vitrine · Cloudflare 自托管图库

配合 [hanabi](https://github.com/Furinelle/hanabi) 使用的二次元图库：

- **存储**：图片进 R2，元数据/标签进 D1  
- **入库**：`POST /api/ingest`（Bearer token + multipart）  
- **浏览**：Workers 静态页，按 **来源** / **标签** / 关键词筛选  
- **标签**：使用帖子自带 tags（Pixiv/X），不做 AI 打标  

## 版本与部署状态

当前源码版本 **v0.2.0**，通过 GitHub Release 提供源码；不是 npm 或 crates.io 软件包。包括公开图库、同源管理后台、可撤销审核、Oracle 相似图计算及原生流式入库。发布记录见 [部署与验收](docs/public-release-2026-10-08.md)。

2026-10-11 v0.2.0 已部署到生产 Worker `69de2943-9efd-4958-ad94-4d7c6251ddef`，线上 WASM 与该版本 release 构建逐字节相同，三条临时入库计时日志已移除。部署保留原 D1/R2、变量、密钥、限速与运行时设置，没有远端迁移；Oracle 分组器及脚本与源码一致，无需重建或重启。GitHub 源码发布与生产部署仍是独立操作。

## 部署

```bash
cd vitrine
npm ci

# 1) 创建 D1 + R2（账号内执行一次）
npx wrangler d1 create vitrine
npx wrangler r2 bucket create vitrine-media

# 2) 把输出的 database_id 填进 wrangler.jsonc 的 d1_databases[0].database_id
#    新部署同时把 database_name / bucket_name 改成上面的资源名。
#    现有 gallery.fontaine.blue 生产环境保留原 D1/R2 名称，避免迁移数据。

# 3) 远端建表
npm run db:remote

# 4) 入库密钥
npx wrangler secret put INGEST_TOKEN
# 粘贴一长串随机 token

# 5) 部署
npm run deploy
```

生产入口为 `https://gallery.fontaine.blue`，写到 hanabi：

```toml
[gallery]
endpoint = "https://gallery.fontaine.blue"
# token 建议放环境变量 HANABI_GALLERY_TOKEN，勿提交仓库
token = ""
```

```bash
export HANABI_GALLERY_TOKEN='与 INGEST_TOKEN 相同'
```

## API

### `POST /api/ingest`

Header: `Authorization: Bearer <INGEST_TOKEN>`  
Body: `multipart/form-data`

| 字段 | 说明 |
|---|---|
| `meta` | JSON：`source`, `source_id`, `source_url`, `title`, `author_name`, `author_url`, `tags[]`, `is_r18`, `origin` |
| `files` | 一张或多张图片（可重复字段名） |

可带 `Idempotency-Key`；相同 key 与内容重试复用已提交回执，不同内容返回 409。R2 上传完成后，元数据、成功审计及回执在同一 D1 batch 提交。如果提交确认丢失，会补读本次成功审计或现有回执；仅确认成功才返回成功，补读失败仍返回失败且保留原图。D1 错误无法可靠区分回滚与提交后响应丢失，因此失败尝试可能留下未引用的 R2 对象，不会自动清理这些结果不明的对象；调用方应使用原 key 重试。此处使用普通 D1 绑定，按 [D1 文档](https://developers.cloudflare.com/d1/worker-api/d1-database/) 读取 primary，不启用读副本 Sessions。

入库限制：最多 40 个文件、单文件 50 MiB、单次请求总计 100 MiB。可选 `telegram_publication`（`chat_id`、`message_ids`、`publish_state=full|partial`）会写入 D1 `telegram_publications`。

### `GET /api/catalog`

Header: `Authorization: Bearer <INGEST_TOKEN>`

列出仍在线的作品图片。批量同步使用 `after=<image id>`（首批为空）主键游标，响应含 `catalog_id` 与 `next_cursor`；`fingerprints=1` 同时返回当前指纹的规范化 JSON SHA-256 摘要 `fingerprint_digest`，缺失时为 null，供 Hanabi 只上传变化项。未带 `after` 的旧 OFFSET 协议保留兼容。可加 `work_id=<source:id>` 只读取该作品的完整分页组，供 Hanabi 相似图审批展示图库原图。现有 `POST /api/catalog/prune` 按图片 key 做可恢复删除；同一 `decision_id` 仅允许相同保留 key 和删除集合重放（集合顺序可变），更换计划返回 409 且不删除对象。

### 目录快照与离线差异

`tools/catalog_snapshot.py` 复用目录 Bearer API，只 GET 元数据，不下载媒体、不写 Worker/D1/R2。凭据沿用 `HANABI_GALLERY_TOKEN` 环境变量，或显式指定已有 env 文件（不在命令行传 token）：

```bash
python3 tools/catalog_snapshot.py snapshot --env /path/to/existing.env --output catalog-before.jsonl
# 下一次核验时保存另一份完整文件
python3 tools/catalog_snapshot.py snapshot --env /path/to/existing.env --output catalog-after.jsonl
python3 tools/catalog_snapshot.py diff catalog-before.jsonl catalog-after.jsonl > catalog-changes.json
```

可用 `--endpoint https://your-gallery.example` 指定 HTTPS origin。HTTP 仅允许 loopback fixture；不跟随重定向。`diff` 完全离线，不需要凭据，按 `catalog_id` 返回新增、移除及字段变化与数量摘要。比较字段限于现有 catalog 的作品/来源/标题、图片 key/页码/大小/类型/SHA-256 和指纹摘要；不会泛化导出额外 API 字段。

JSONL 按图片 ID 严格递增，每份文件含格式/来源/版本头、图片行及数量/SHA-256 完整性尾。读完所有游标且前后 generation 相同才原子替换目标文件；重复 ID、异常游标、请求/解析失败、超限或版本变化都会清理 owner-only 临时文件并保留旧文件。摘要只覆盖图片行，便于核对内容。被 SIGKILL/断电中止时可能遗留 `0600` 的 `.partial` 文件，旧完整文件不会被提前替换；这些残片不能通过完整性校验，可手动清理。`diff` 拒绝截断、摘要/数量不符或来源不同的文件。

前后 generation 检查依赖现有变更触发器，并非跨请求数据库事务；持续入库时可能需要稍后重试，校验后发生的新变更会留到下一份快照。此工具不验证 R2 对象是否存在或媒体字节是否正确，也不是媒体、Telegram mapping 或完整数据库备份。每页最多 100 项，另有两次轻量版本查询；单次响应上限 8 MiB、图片行总量上限 128 MiB。

离线回归：`python3 tools/test_catalog_snapshot.py`（临时文件及本地 HTTP fixture，无真实凭据/生产访问）；也包含在 `npm run test:offline`。

### `PUT /api/catalog/fingerprints`

Header: `Authorization: Bearer <INGEST_TOKEN>`

Oracle / Hanabi 指纹同步写入感知哈希（hex，最多 50 条）。相同内容为真正 no-op，不更新 timestamp、不增加版本；`upserted` 仍表示接受条数。CLIP PUT 同样跳过内容相同的写入。Worker 不解图。路径在 `/api/catalog` 下，不会被 `/admin` WAF Challenge 挡住。

### `GET /api/admin/similar` · `POST /api/admin/similar/delete`

Cookie。读取 Oracle 预先计算的相似结果；Worker 不再现场计算或聚类。目录变化后，Worker 只按缓存候选的 key 核验当前在线成员、特征与保留项；仍有效的旧候选可继续审批，无法核验的候选暂不显示。新增候选由下一轮 Oracle 扫描发现；尚无可用结果时返回 `pending: true`。相似页按张去重。删除请求必须包含 `items: [{r2_key, decision_id}]`，每项 `decision_id` 为稳定的 `sim-UUID`，重试复用原 ID，并携带该组未选择删除的 `keep_r2_keys`。删除事务校验保留图片的 key、作品和版本，避免并发/迟到计划把最后一张也删掉。响应逐项返回 `results` 与 `partial/deleted/failed`，客户端保留未完成批次。Telegram 失败或无法确认时返回精确 targets，不将其报告为已删除。删除走 `image-review` 回收站；有频道 mapping 时 Worker 用 `TELEGRAM_BOT_TOKEN` 调 `deleteMessage`。Bot 失败不回滚图库删除。`GET /api/admin/fingerprints/status` 看覆盖率。

### Oracle 相似图计算

`GET /api/catalog/similar/snapshot` 默认只读取版本和结果是否最新，`?counts=1` 才读取按版本缓存的输入数量；`GET /api/catalog/similar/input` 用 `after` / `next_cursor` 游标按版本分页导出 fingerprints / clip；`POST /api/catalog/similar/result` 原子接收该版本的完整结果，过期返回 409。三个接口都只接受已有 catalog Bearer 凭据，不对访客公开。结果请求上限 4 MiB。

`cargo build --release --locked --bin vitrine-similar` 构建原生计算程序，复用与 Worker 旧实现相同的严格去重和相似聚类算法。Oracle 的 `tools/similar_compute.py` 每天北京时间 08:15 运行，先检查版本；没有变化且已完成扫描时直接结束。有变化时补齐缺失 CLIP（Oracle 四核），再读取输入并计算分组。后台统计按数据版本缓存，等待结果每 5 分钟检查一次；D1 日额度耗尽时停止自动重试。部署、资源上限和回滚见 [Oracle runner](deploy/oracle-similar/README.md)。

### `PUT /api/catalog/publications`

Header: `Authorization: Bearer <INGEST_TOKEN>`

回填精确的频道 publication 映射。仅接受在线作品；相同 payload 幂等，冲突返回 409。

### `POST /api/catalog/retract`

Header: `Authorization: Bearer <INGEST_TOKEN>`

按作品 ID 撤回刚入库的一条作品：备份 R2 到 `review-trash/<decision_id>/...`，删除 images/work_tags，软删除 works 与 telegram_publications。不要求 keep 作品，也不要求 Telegram mapping。已撤回且没有当前图片的作品返回 `replayed=true`；仅人工下线仍须完成撤回。供 Hanabi `/undo` 撤回「发送并入库」。

### `POST /api/catalog/prune-works`

Header: `Authorization: Bearer <INGEST_TOKEN>`

按整部作品删除相似审核中落选的作品：先备份 R2 到 `review-trash/<decision_id>/...`，再在同一 D1 batch 中写入 `catalog_work_prune_receipts`、删除 images/work_tags、软删除 works。每个落选作品必须已有完整的在线 Telegram mapping，否则 409 且不做任何变更。同一 `decision_id` 重放会重试删除已记录的原 R2 key 并返回存储的 Telegram targets。

Hanabi 拿到 targets 后删除对应频道消息，再调用 `POST /api/catalog/prune-works/telegram-result` 把 receipt 标为 complete。没有 mapping 的历史作品只能先精确回填，不能走这条破坏路径。现有 `/api/catalog/prune` 图片 key 接口保持兼容，但相似审核不再使用它。

### 后台 `/admin`

同源管理页：改标题/作者/标签/R18，删/恢复单图，下线整部作品（只写 `works.deleted_at`，不动频道），以及「相似图」按张去重。登录用现有 `INGEST_TOKEN`，Worker 发 HttpOnly cookie。频道删帖另需 `npx wrangler secret put TELEGRAM_BOT_TOKEN`。首次部署须按顺序应用迁移 `0001`–`0013`；已有实例先核对实际迁移状态，保留 D1 备份与回滚方案，再由操作者执行 `npm run db:remote`。

WAF Managed Challenge 已加在 zone `fontaine.blue`（规则 id `a7940432beeb4799a74e515a9dd1bf4b`）：

```
(http.host eq "gallery.fontaine.blue" and (
  starts_with(http.request.uri.path, "/admin") or
  starts_with(http.request.uri.path, "/api/admin")
))
```

不要再手建一条同样的。不要把 `/api/catalog`、`/api/ingest`、`/api/works`、`/media` 加进去。`*.workers.dev` 不走这条 hostname 规则；生产只用自定义域。Worker 的 workers.dev / preview 入口已关闭，避免绕过该域名下的 WAF。

### `GET /api/admin/session` · `POST /api/admin/login` · `POST /api/admin/logout`

Login body：`{ "token": "<INGEST_TOKEN>" }`。只认 cookie，不认 Bearer。写操作要求 `Origin` 同源。

### `PATCH /api/admin/works/<URL 编码的 work_id>`

Cookie。可改 `title`、`author_name`、`author_url`、`tags`、`is_r18`。缺省字段不动；`tags` 一旦出现即整表替换。

### `POST /api/admin/works/<URL 编码的 work_id>/hide`

Cookie。只软删除作品并保留图片。已下线返回 `replayed: true`。人工下线会在后续重新入库时保留，必须明确恢复；不要用 `retract` 代替。

### `GET /api/admin/works?status=active|hidden` · `GET /api/admin/works/<work_id>`

仅管理员 cookie 可以读取下线列表和详情。下线媒体只允许管理员读取，响应 `private, no-store`；回收站备份仍不可访问。

### `POST /api/admin/works/<work_id>/restore`

仅恢复手工下线且图片元数据、原图完整性、版本均匹配的作品。不恢复已 prune/retract 的内容，也不复用过期单图快照。重复调用幂等。后台切换作品、关闭编辑器、退出前会保护未保存的修改。

### `GET /api/works?source=&tag=&q=&limit=&offset=&cursor=`

按 `created_at DESC, id DESC` 稳定排序。`tag` 可用逗号分隔多个标签（最多 64 个），返回带有其中任一标签的作品，每件作品只出现一次；单个标签的行为不变。新网页使用 `next_cursor` 继续浏览，避免新入库/下线移动 offset 导致漏项；旧 offset 客户端保持兼容。搜索中的 `%` / `_` 按普通文字处理。

### `GET /api/works/<URL 编码的 work_id>`

公开返回 `work` 元数据及 `images` 完整图片列表，按原页码排序；已下线作品返回 404。公开元数据不含内部采集 `origin`。网页用此接口浏览多图作品，无需把图库管理令牌交给浏览器。

### `POST /api/catalog/image-review`

使用管理 Bearer token **或** 后台 cookie（仅此 catalog 接口认 cookie），提交 `decision_id`、`r2_key` 和 `action=prepare|delete|restore`。后台调用时加 `allow_missing_telegram: true`。`retract` / `prune-works` 仍只认 Bearer。单图删除保留不可变 R2 原图与恢复副本，通过 D1 控制可见性；`/media` 只提供仍在线的图片，备份不公开。图片使用 ETag 重验证，撤销不再与迟到的物理删除竞争。

整作品撤回、清理或重新入库会使旧单图快照失效，返回 409。`restore` 也可取消仍为 `prepared` 的删除；调用方必须等远端确认后再完成本地撤销。迁移 0006 增加作品版本与媒体索引，旧无版本快照保守拒绝。

### `GET /api/tags` · `GET /api/sources`

标签支持 `q`、`limit`（最多 200）、`offset` 和 `order=name`，通过 `next_offset` 遍历全部在线标签；网页索引使用服务端搜索，不限于热门的前 200 个。

### `GET /media/<r2_key>`

可加 `?width=160|640|1280` 读取预设缩略图。原图与缩略图都先检查 D1 在线状态，再用 `private, no-cache` 和 ETag 重验证。浏览器不再直接请求 `/cdn-cgi/image/`；gallery 域名已拦截旧裸变换入口（规则 `e41a69a481324622bd194a9c1eeedd12`），以免旧缓存绕开 Worker。`npm run test:public` 只读验收正式站，包括真实缩图尺寸，避免将返回原图的 200 误认为优化成功。

## 与 hanabi 的行为

| 操作 | 频道 | 图库 |
|---|---|---|
| ✅ 发送到频道 | 发 | 否 |
| 📦 发送并入库 | 发 | 是（帖子自带 tags） |
| ❌ 丢弃 | 否 | 否 |
| `/approve` 一键批准 | 发 | 否 |
| 手动发单作品链接直发 | 发 | 是（若已配置 gallery） |

## 本地开发

Worker 实现已迁到 **Rust / workers-rs**（`src/lib.rs`）。`src/index.ts` 与 `public/` 仍保留作对照与前端静态页。

```bash
# 依赖
npm ci
# 需已安装 Rust + wasm32-unknown-unknown + worker-build
# rustup target add wasm32-unknown-unknown
# cargo install worker-build

npm run db:local    # 仅本地应用 D1 迁移 0001–0013
npm run build:dev   # worker-build --dev → build/worker/shim.mjs
npx wrangler dev

# 质量检查（不部署）
CARGO_BUILD_JOBS=2 npm run check  # Rust、前端、Oracle 离线回归和 WASM 构建
CARGO_BUILD_JOBS=2 npm run test:recovery  # 实际 WASM 的提交确认丢失、重试和删除回执故障注入
CARGO_BUILD_JOBS=2 npm run test:integration  # 独立临时 D1/R2 的完整 HTTP 回归
npm run build       # release Worker 构建；不部署
```

开发只使用 `npm run db:local`。`0004_telegram_publications.sql` 建立频道映射及整作品清理回执，后续迁移补齐单图审核、特征、相似缓存、目录版本和查询索引。

`npm run db:remote` 仅由获授权的操作者执行。已有生产实例须先导出/备份 D1、核对目标和待应用迁移；本地开发和自动回归均不得使用远端迁移。

## 公开访问版本

- 公共页面使用独立 `public/gallery.css`；管理后台继续使用既有 `public/styles.css`，避免公开页重构影响管理布局。
- 稳定的按行网格、固定图片占位、自动分页、筛选浏览历史、作品与页码分享链接、键盘/触摸切图、深浅外观、失败重试及标签索引。公开页统一展示作品，不展示历史 R18 标签；数据库标记保留。
- 前后台共用 theme.css / theme.js。悬浮控件以 WebGL 采样视口内已加载的同源缩略图，沿胶囊边缘折射；未采样文字使用 CSS 背景模糊。无 WebGL 时保留 CSS 材质；减少透明度/高对比时使用实色。图库本身不加模糊、不裁切。
- 登录按 IP/Cloudflare 机房限制 5 次/分钟，公开列表/详情/标签/来源 API 为 120 次/分钟。该机制并非全局硬配额，不能替代 Cloudflare DDoS 防护。配置绑定必须随 Worker 一起部署。
- 静态资源与 Worker 响应均提供 CSP、防 iframe、nosniff、Referrer/Permissions Policy 与 HSTS；后台 cookie 使用 HMAC-SHA256、HttpOnly、Secure（HTTPS）、SameSite=Lax。更新后旧会话需要重新登录，入库 Bearer token 不变。
- JSON 写入上限 1 MiB，登录 16 KiB，包括无 Content-Length 的分块请求；入库保留现有 40 文件/50 MiB 单文件/100 MiB 总量，读取内容前先验文件大小。只将 12 字节魔数读入 WASM，以原生 DigestStream 计算 SHA-256，并将原生文件流上传 R2；multipart 解析仍由 runtime 管理。免费 Workers 的 CPU 上限仍可能使大请求报 1102，本地大文件通过不代表生产配额问题彻底消失。

本地回归使用可丢弃的独立 D1/R2 state，**不得指向生产**：

```bash
npx wrangler d1 migrations apply DB --local --persist-to /tmp/vitrine-test-state
npm run build:dev
npx wrangler dev --local --ip 127.0.0.1 --port 18787 \
  --local-upstream 127.0.0.1 --upstream-protocol http \
  --persist-to /tmp/vitrine-test-state --var INGEST_TOKEN:local-image-review-test
# 在另一个终端
VITRINE_TEST_STATE=/tmp/vitrine-test-state python3 tools/test_image_review.py
python3 tools/test_public_security.py
python3 tools/test_admin_security.py
node tools/test_frontend.mjs
```

使用锁定的 Wrangler 4.148.0；4.114.0 的本地代理会在分块请求被提前拒绝后退出，不能用来验收此边界。测试 token 仅用于本地。Cloudflare 真正图像变换需部署后验证尺寸、字节数、ETag 与可见性；loopback 开发服务只返回原图。迁移范围以仓库和部署记录为准；不要把 GitHub 源码发布当作已应用生产迁移。

开发依赖 `sharp` 暂锁补丁版 0.35.5，以修复 [GHSA-wq5f-xc86-pv6w](https://github.com/advisories/GHSA-wq5f-xc86-pv6w)。待 Wrangler/Miniflare 上游采用修复版本后可移除 override。此依赖不进入生产 Rust Worker。

输入框、按钮与正文使用正常中文系统字体（苹方/微软雅黑等）；标题采用 Ma Shan Zheng，字标采用 Cormorant Garamond。约 38 KiB 的固定文字子集随站点托管，OFL 许可证见 `public/fonts/`；未包含的动态标签回退到系统字体。浏览器无需连接 Google Fonts。

完整 HTTP 验收统一运行 `npm run test:integration`：自动创建独立配置、临时 D1/R2、测试令牌，不加载项目 `.dev.vars`，执行浏览/安全/后台生命周期/去重并发/撤销/目录增量协议/96 MiB 流式入库回归，完成后清理测试数据。相似结果由 Oracle 上的原生 Rust 程序计算；D1 写入触发器递增数据版本，分页导出和结果发布都校验同一版本。旧候选须通过当前特征和保留项核验才能删除；删除事务仍校验同一目录版本，已完成回执可继续重放。

公开标签和来源统计在 Cache API 中按目录版本保存；标签查询在同一份完整统计上筛选、排序、分页，避免每次搜索重做全库聚合。作品隐藏/恢复及元数据修改使缓存失效。`0013_catalog_read_cost.sql` 追加可见作品排序索引与直接 work_tags 修改的失效触发器；正式迁移状态以部署记录为准。

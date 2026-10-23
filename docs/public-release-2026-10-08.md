# 2026-10-08 公开访问版本验收

正式站：https://gallery.fontaine.blue/

Worker 版本：`0c7031d7-e1c1-40e5-a6a8-22592b735469`。
进度重试修订前版本：`8fc5ad96-4d0d-47b2-b561-a96e30a5fc97`。
Oracle 计算迁移前版本：`bc157b67-62dd-4731-814f-47a9636f47b7`。
后台布局统一前版本：`63c05b7a-8b93-46cb-96e4-21200fc1de18`。
滚动交互修订前版本：`326a8620-fdb2-463b-80cb-ad60e3c7a7ba`。
本轮玻璃/后台修订前版本：`1b7d8aac-80c2-4e3f-85d8-741ad6e4c104`。
本次公开改版前的正式版本：`2ec2f26f-2295-4edf-8761-f05e105eaf12`。
默认浅色调整前的版本：`0cffe0cd-cb49-476d-b635-5a281eb538d7`。

最初公开改版没有生产 D1 迁移；后续 Oracle 计算迁移新增了 0012 版本表与触发器。全程没有生产作品编辑或图片删除。首次公开安全改版验收时为 2,781 部在线作品、62 部下线作品、4,312 张在线图片；正常入库持续进行，这不是实时计数。

## 已交付

- 公开页改为按行排列的响应式网格，图片预留空间；字体与玻璃面板调整，图像保持主体。公共页不显示 R18 标签。
- 默认外观改为浅色，包括 CSS 和浏览器主题色；主动选择的深色偏好仍保留。Chrome 新访客样本为 light，切换到 dark 后刷新保持，再切回 light 正常。
- 搜索、来源和标签筛选、自动分页、错误重试、多图浏览、键盘与分享链接保留。游标分页避免入库/下线时漏项；全部标签支持服务端搜索和分页。
- 管理员可以查看下线作品并恢复完整的人工下线作品。重新入库保留人工下线状态；撤回与恢复使用版本及原图完整性保护；未保存编辑有离开确认。
- 登录签名 cookie、CSRF、请求体限制、上传格式与大小校验、安全响应头、登录和公开接口限速。公开响应移除内部采集 origin。
- 相似结果由 Oracle 按数据版本预计算；并发删除在同一 D1 事务检查保留图片及版本，避免双方把最后一张也删掉；失败项目可按稳定 ID 重试。
- 原图和缩略图都经 D1 可见性校验；关闭 workers.dev 与 preview 入口；R2 保持私有；保留后台 WAF Challenge。（2026-10-23 起不再成立，见文末「2026-10-23 现状更正」）

## 验证证据

- `npm run check`：64 项 Rust 测试、Clippy、前端回归、JS 语法与 WASM 构建通过；最终缩图修正再次通过 Clippy 和 release 构建。
- `npm run test:integration`：全新临时 D1/R2 上的五组 HTTP 回归通过，包括分块请求大小限制、CSRF、隐藏恢复、重新入库、并发删除、旧审核回执与缓存失效。未接入生产资源。
- `npm run test:public`：正式站只读检查通过。首页安全头、未授权 catalog 401、游标和完整标签索引、三档真实缩图尺寸、ETag 304，以及旧缩图入口 403。
- 样本原图 1,351 × 1,431 / 498,467 B；WebP 缩图宽度 160 / 640 / 1,280，分别 10,602 / 87,370 / 238,488 B。已修复 workers-rs 的数值质量参数序列化导致静默回退原图的问题，使用平台默认质量。
- 既有下线作品：公开详情、原图及三档缩略图均为 404，带条件缓存请求也不能绕过。
- workers.dev 与版本预览 URL 实测 403；R2 r2.dev 禁用且没有公开自定义域。（2026-10-23 起不再成立，见文末「2026-10-23 现状更正」）
- Oracle 上实际 Hanabi 容器部署后只读检查通过：health、Bearer catalog、来源/ID 作者查找、原图和新缩略图均正常。凭据只在容器内读取，未输出；没有生产上传、写入或 Telegram 操作。记录在 `/tmp/vitrine-oracle-readonly-acceptance-20261008.jsonl`。
- 手机正式页无横向溢出；分享链接能直接打开第 2 张图片。桌面和 320/390 宽度布局、自动分页 24 → 48、多图键盘与筛选在本地样本验收通过。
- 后续按用户要求在 Chrome 正式页复验：点击打开多图、下一张及左箭头、Escape 关闭、无结果搜索与恢复、完整标签搜索及筛选、自动加载 24 → 48（无重复）、320/390 双列及无横向溢出，均通过。分享 URL 随页码更新，复制按钮显示成功；系统剪贴板内容未独立确认。正式页控制台未见 error。
- Chrome 本地后台隔离样本复验：登录、保存修改、未保存离开确认的取消和接受、下线、恢复公开均成功；样本已恢复在线状态和原始标题。原内嵌浏览器阻塞未再作为验收依据。
- npm audit：0 条漏洞。RustSec：96 个锁定依赖、1,294 条公告，0 条漏洞及警告；公告库 commit `b8a1a33e246a0a9a3b5f377248c41a503defec74`（2026-10-07），未做 yanked 检查。这不是无漏洞保证。

## 玻璃材质、后台与重复核查补充

- 移除仅部分浏览器执行的 SVG backdrop 引用滤镜及日文圆体/圆体优先字体。输入框、按钮和正文统一使用系统中文字体。
- 前后台共享 theme.css / theme.js / glass.js。独立胶囊和圆按钮使用局部 WebGL 图像折射、CSS 背景模糊与高光；只读取已加载同源缩略图，不截图或复制整页，不增加图片请求。滚动/尺寸/图片变化才绘制，视口外图片不逐帧处理。
- 正式站已在 Mac Safari 和 Chrome 观察到图像沿圆角边缘折射。Chrome 390/320 布局在本地复验无横向溢出。未做 iPhone 真机性能/滚动验收；网页近似不是 Apple 系统原生材质，背景文字不做精确光学折射。
- 后台统一字体、主题、表单和相似组布局，增加独立保存/未保存/处理中状态，手机编辑区移到列表上方，关闭后回到所选卡片；去掉容易误解的退出图标。Chrome 用隔离数据验证手机选中编辑、取消未保存离开、深浅主题切换和相似页导航。
- 正式库最新 48 封面：1128 对比较，字节相同和视觉 StrictSame 都是 0 对。全库有 11 组跨作品字节相同的图片（常见于合集复用单图），另有 2 组同作品重复页；整部作品的有序图片 SHA 序列完全重复为 0。它不排除语义/局部相似。
- 之前本地预览为 8 个样本重复 6 组，造成误解；已只保留 8 件代表作品在线，并将 Safari 展示页切回正式站。正式库没有 preview ID。
- 入库目前按来源 ID 幂等，后台生成去重建议后需确认执行，并非后台自动删除任务。一键建议现严格要求重新计算 StrictSame，CLIP 高分本身不能进入清理建议；缓存策略版本已更新，避免复用旧分类。
- 本轮 npm check、五组隔离集成回归、正式站只读检查通过。新增共享主题独立启动、图片 contain 采样、后台状态及取消时焦点保护检查。没有生产图片/作品删除，也没有 Telegram 操作。

## 滚动交互与缩略图补充

- 公共封面统一 1:1、`object-fit: cover` 居中裁切并移除框线；详情继续 contain 展示完整原图。后台审核缩略图保持完整显示。
- 前后台搜索框去掉额外外描边，保留原边框变色的焦点提示及原生清空功能，显式重置 WebKit search appearance。
- 公共顶部控件改成固定定位，保留布局占位和安全区；聚焦输入时按 VisualViewport.offsetTop 跟随可视区域，捏合缩放不强行跟随。软键盘路径使用模拟回归检查，尚未实测 iPhone。
- 400px 后显示浮动回到顶部按钮；点击后焦点回到内容、滚动到 0，保留未提交搜索字串；遵循 reduced-motion。
- 事件驱动的 WebGL 画布保留上一帧绘图缓冲，避免呈现后丢失画面；采样同步支持实际 cover/contain。仍按事件合并单次 RAF，没有新增常驻动画循环。
- 前端、玻璃与后台 UI 回归、JS 语法、diff 和 release 构建通过。本地 Chrome 390px 验证搜索输入后滚动、回到 0、搜索成功/无结果、完整原图和深色布局。
- 正式站 Chrome 桌面、320/390px 无横向溢出；390px 聚焦搜索后连续滚动到 1444 / 2034.5 / 2625.5px，搜索栏 y=12px、草稿和焦点保持，分页 24 → 48。回顶最终 scrollY=0；控制台无 error；正式只读检查通过。Mac 解锁后补完原生 Safari 正式页验收：刷新确认方形封面和新版回顶入口；输入“芙芙”后连续三段滚动，焦点、草稿和搜索栏保持可见；回顶后滚动条为 0；提交搜索得到 10 件结果，深色聚焦/滚动正常，最后恢复浅色和全部作品。滚动前后截图未见玻璃层丢失；未进行逐帧录像或 iPhone 软键盘真机验证。截图为 `vitrine-scroll-live-safari.png` 和 `vitrine-scroll-live-safari-dark.png`。
- 本轮仅前端文件改动，备份/发布日志 `/tmp/vitrine-scroll-20261008/`；单独回退可执行 `npx wrangler rollback 326a8620-fdb2-463b-80cb-ad60e3c7a7ba`，不改 WAF 和数据。

## 后台与主页布局统一

- 之前只共用材质与颜色，后台仍采用左侧导航、小图列表和常驻空编辑栏；本轮移除这套独立布局，采用主页面的浮动标志/搜索/主题顶栏、横向导航、书写体栏目标题、方形大图网格和留白。
- 顶栏 CSS、响应式尺寸和搜索框可视视口处理直接移入共享 theme.css / theme.js，主页面和后台都使用同一套规则；后台登录页仍可切换外观。
- 未选中作品时编辑栏完全收起，选中才展开；桌面旁列编辑，980px 以下编辑区移到列表前并获得焦点。列表封面 cover，作品图片和相似图审批继续 contain，避免审核时遗漏边缘。
- 本地 Chrome 桌面和 320/390px 验证无横向溢出，来源筛选、登录退出、编辑器开关、未保存提示取消/接受、相似图导航正常；主页共享顶栏回归保持位置和未提交输入。
- 三组前端回归、JS 语法、diff 和 release 构建通过；本轮未改后端或生产作品数据。备份与日志 `/tmp/vitrine-admin-unify-20261008/`。单独回滚：`npx wrangler rollback 63c05b7a-8b93-46cb-96e4-21200fc1de18`。

## Oracle 计算迁移（用户确认）

- 正式相似接口曾返回 Cloudflare Error 1102，单次受控复现的 Worker tail 明确为 `exceededCpu`。原因是特征虽在 Oracle 提取，Worker 仍读取全库特征、做摘要和聚类。
- 现在 `vitrine-similar` 在 Oracle 原生运行同一 Rust 严格相同/感知哈希/CLIP 算法；Worker 的列表和删除请求均不再全库比较。已删除 Worker 全库 loader，原生计算入口仅由 CLI 调用。
- 新增 `0012_similar_generation.sql`，works/images/fingerprints/clip 写入触发 12 个版本触发器。Oracle 分页读取统一版本，数据改变则拒绝后续分页或发布；发布用同一 SQL 条件原子校验，不覆盖新版本结果。
- 所有快照/导出/发布接口只接受 catalog Bearer，发布上限 4 MiB。过期结果在页面显示“等待 Oracle 更新相似图结果”，只有可见后台页面轮询。新删除等待新结果；已有成功回执仍可重放，原保留图片/版本/事务保护保留。
- Oracle：`/home/ubuntu/vitrine-similar/current` → `releases/20261008-oracle-1`；Rust 1.95 容器在 aarch64 目标本机构建，运行 ELF64 aarch64 原生程序。systemd 的 `vitrine-similar.service` 使用 ubuntu、1 CPU 上限、2 GiB 内存上限，直接读取既有凭据文件，不复制密钥。
- `vitrine-similar.timer` 每轮结束约 60 秒后加最多 15 秒抖动检查；版本未变只请求状态。单实例锁避免重叠；失败保留既有结果，过期结果不继续用于新删除。
- 首轮真实运行：generation=1，4,315 fingerprints / 2,402 CLIP，成功 published；日志记录整轮 1.483 秒 CPU、39.3 MiB 峰值内存。第二轮 `generation=1 current`，跳过导出和计算。认证 snapshot 返回 `result_fresh=true`、`result_generation=1`。
- 正式后台从等待状态自动显示 35 组相似候选及 10 对严格同图建议。没有执行清理或删除。迁移验收时仍有 1,913 张 CLIP 未提取，当时未启动回填；后续补扫记录见下一节。该时点使用感知哈希结果。
- 67 项 Rust 测试、Clippy、前端三组回归、8 项运行器检查、五组隔离 HTTP 集成回归通过。HTTP 覆盖匿名拒绝、过期分页/发布、待计算 UI、旧回执重放、并发/迟到保留保护；正式站公开只读检查通过。生产仅对结果缓存和新版本表写入。
- 标题字体已补齐“相似审批管理”等原子集缺字；前后台共用回顶逻辑与控件样式。Chrome 正式后台桌面/390px、编辑只读打开关闭、审核图片 contain、回顶和无横向溢出通过。
- 备份：`/tmp/vitrine-oracle-similar-20261008/` 内有 38,518,866 字节 D1 SQL、迁移前源码、构建和发布记录；远端 rollback 目录保留新服务安装前状态。新表为追加迁移，回滚不需要覆盖作品数据。

回退此次计算迁移：先在 Oracle 停止并禁用 `vitrine-similar.timer` 与 service，再执行 `npx wrangler rollback bc157b67-62dd-4731-814f-47a9636f47b7`。保留追加版本表/触发器；不要用旧整库备份覆盖迁移后的正常入库。回退会恢复旧的 Worker 现场聚类及其 CPU 超限风险。

## CLIP 自动补扫（2026-10-08 后续）

- 追查发现旧 `clip_embed.py` 在 9 月 15 日完成一次性扫描后没有持续调度，Oracle 分组任务此前不负责补齐特征。这导致 1,913 张在线图片缺少 CLIP 向量。
- 已把缺失图片扫描接到同一 `vitrine-similar.service`：先查 missing，只处理缺失图片并分批确认上传，然后生成/发布分组；没有缺失时不加载模型。沿用原 ViT-B-32/openai 与原图预处理，不混用新模型或压缩缩略图。
- 复用 `/home/ubuntu/vitrine-clip/venv` 与已缓存模型，离线加载模型；初次试跑使用单线程，随后按用户要求改为 4 个 Torch 线程、4 核 CPU 上限和 2 GiB 内存上限。与分组共用单实例锁，避免两个任务互相使数据快照过期。
- 图片与 API 请求有有限重试/大小上限；已上传进度存于 D1，下次只补缺失项。扫描出错或超时仍尝试发布已完成批次的分组，并让下轮重试失败图片；未扫描完不标记完成。
- 两张真实图片试跑 verified=2、failed=0，missing 从 1,913 降到 1,911。14 项扫描器/有界预取测试与 11 项运行器/顺序/部分失败测试通过。四核预取实测每 20 张约 13 秒，原单核串行约 31 秒；重启从缺失项继续，不重扫已上传向量。
- 当前 release `/home/ubuntu/vitrine-similar/releases/20261008-scan-4core`，原生分组二进制不变。下载与推理交叠，最多预取 4 张原图（约 200 MiB 上限），不提交整库下载、不并行加载多个模型。先前脚本、unit 与 missing 键清单保存在远端 `rollback/20261008-before-clip/`；本地日志与源码备份 `/tmp/vitrine-clip-scan-20261008/`。扫描计算部署在 Oracle；同时修复后台等待进度遇到一次临时请求错误后永久停刷的问题，等待期间保持可见页面的 5 秒重试，401/离开视图仍停止。

- 补扫最终完成：旧特征 2,402 张，本轮新增 1,913 张（两张试跑、单核阶段 280 张、四核阶段 1,631 张）。最后一轮 `verified=1631, failed=0, skipped=0, remaining=0`；全库 CLIP 4,315 / 4,315。
- Oracle 已发布 generation=1914，认证快照 `result_generation=1914, result_fresh=true`；正式后台显示 69 组 CLIP 相似候选、10 对严格去重建议。没有执行任何删除，也未人工逐组审核。
- 四核阶段 service Result=success / ExecMainStatus=0，CPUQuotaPerSecUSec=4s，日志峰值内存 1.4 GiB。随后一次自动调度仅输出 `generation=1914 current`，没有重新加载模型或重扫图片；timer 继续 active。
- 正式页面截图：`vitrine-clip-scan-complete.png`；后台等待过程中临时失败会继续重试，新脚本 `admin.js?v=20261008-scan` 已确认生效。

## 运行边界

- Telegram 删除失败仍保留精确目标和回执供核实，没有新增自动删帖重放。
- 限速按 Cloudflare 节点计数；已下载到访客设备的公开图片无法撤回。

## 回滚

当前源文件、原工作树 diff 和发布前配置保留在 `/tmp/vitrine-before-public-20261008-104036/`。验收日志在 `/tmp/vitrine-public-acceptance/`，临时路径应在需要长期保留时另行归档。

仅回退本轮玻璃/后台修订时，在项目目录执行（无需改 WAF）：

```sh
npx wrangler rollback 1b7d8aac-80c2-4e3f-85d8-741ad6e4c104
```

若回退到更早的公开改版前版本 `2ec2f26f-2295-4edf-8761-f05e105eaf12`，旧前端使用 `/cdn-cgi/image/`，因此完整回退还需停用本次新增的 WAF 规则 `e41a69a481324622bd194a9c1eeedd12`（ruleset `53bafaa2128745a2b588a2a77082a847`）。原后台 Challenge `a7940432beeb4799a74e515a9dd1bf4b` 现已删除。回退不需要迁移或恢复作品数据；旧版重新带回旧的缓存可见性限制。

## D1 读取额度修复与每日扫描

- 已部署 Worker `e24952a9-da9c-463a-a4d7-bdc45f71724b`，替代 `0c7031d7-e1c1-40e5-a6a8-22592b735469`。没有新增数据库迁移或更改作品数据。
- snapshot 默认只读版本/结果元数据；需要导出时才用 `counts=1`。统计结果由 Cache API 按 origin、generation 缓存，D1 前后版本校验防止混用；缓存失效仅影响成本，不参与删除授权。
- 导出改为主键游标 `after` / `next_cursor`，Oracle 同步升级；非零 OFFSET 被拒绝。30,000 条隔离 SQLite 验证所有在线记录恰好导出一次，深页 VM 步数由 388442 / 387042 降为 3212 / 1812。该数值是本地 SQLite VM 工作量，不是线上 D1 计费行数。
- 用户要求一天一次，timer 改为每天 00:15 UTC（北京时间/新加坡 08:15），最多 15 秒抖动；保留 Oracle 四核、2 GiB 内存。线上下一次触发已确认 `2026-10-09 00:15:05 UTC`。发布目录为 `/home/ubuntu/vitrine-similar/releases/20261008-d1-reads`，二进制和 CLIP 模型保持不变。
- runner 记录成功扫描开始前的版本；未变化且结果新鲜时仅一次轻量请求。扫描中发生变更会留下下一轮检查机会；失败不标记完成。日额度错误不立即重试，并可跨进程保留到重置时刻的等待状态。
- 后台等待相似结果改为每 5 分钟查询，错误指数退避，日额度耗尽停止自动刷新。服务端统一提供 503 / database_daily_limit / Retry-After；入库失败先清理本次临时上传，再传递同一额度错误。Chrome 已确认显示中文额度说明。
- 校验：68 Rust 测试通过（1 性能测量忽略）、Clippy/fmt、17 runner 测试、14 CLIP 测试、全部前端测试、5 组隔离 HTTP 集成回归通过。生产 Oracle 的新轻量 snapshot 返回 generation=1914、result_fresh=true；正式后台列表仍返回日额度错误，因此不把入库恢复标为完成。
- Hanabi 只读检查确认本次图库失败项处于 pending，2 个重试文件均存在且频道发送记录存在。原有持久退避重试保留；未重发频道、未手动重跑扫描、未修改 Hanabi 队列。
- 回滚材料：`/tmp/vitrine-d1-reads-20261008/` 保存旧源码、构建/回归/部署记录；Oracle `rollback/20261008-before-read-fix/` 保存旧 unit 和 current 目标。需要回滚时先停 timer、同时恢复匹配的 runner/Worker API，避免旧 OFFSET 客户端调用新游标接口。回滚会恢复原先重复读取问题。

## 剩余读取放大修复（索引等待额度重置）

- Vitrine Worker 已部署 `83c03134-06af-410c-b303-96dcf7d2ab75`，前一版本 `e24952a9-da9c-463a-a4d7-bdc45f71724b`。
- 指纹和 CLIP 的 UPSERT 仅在内容变化时 UPDATE；相同内容连 updated_at 也不修改，upserted 仍表示接受条数，保持调用方确认协议。没有放宽任何删除/恢复/鉴权保护。
- 新 `/api/catalog?after=&limit=100&fingerprints=1` 使用 image ID 主键范围，返回 catalog_id、next_cursor、fingerprint_digest（远端缺失为 null）。摘要是 fingerprint_upload_json 的 compact sorted serde_json Value 的 SHA-256；双方黄金向量与真实隔离 HTTP 交叉测试一致。旧 work_id/OFFSET 调用仍兼容。Oracle 对正式新接口单页验证成功，返回合法 cursor 与 64 位摘要。
- 标签和来源按现有目录 generation 缓存；标签搜索和分页复用完整统计。公共列表用 tuple cursor 和 tag-index 驱动 JOIN。69 Rust 测试通过（1 性能测量忽略）、Clippy/fmt、6 组隔离 HTTP 集成测试、前端测试通过。独立备份测试比较 90 组搜索/排序/分页语义一致。
- `0013_catalog_read_cost.sql`（两个 partial index、三个 work_tags generation triggers）**尚未在生产应用**：2026-10-08 15:49 UTC 的 Wrangler migrations 请求被 D1 日读取额度 7500 拒绝。当前 Worker 不依赖新列/表即可运行；索引收益和直接修改 work_tags 的额外失效保护须迁移后验收。现有应用标签编辑本身也更新 works，继续走既有 generation 失效。
- 用户明确授权 2026-10-09 08:05 北京时间/新加坡时间自动继续。一次性 heartbeat `automation`，当前线程 `01a1195c-aa47-7222-9e68-6f0117a4e7af`，先应用迁移、验收缓存/小规模读成本，再检查 Hanabi 的持久 pending 作品 2108166444232135049；仅图库补入库，不重复发送 Telegram。
- 本地备份/证据 `/tmp/vitrine-quota-fix2-20261008/`。Oracle `/opt/hanabi-container/backups/20261008-quota-fix/` 已保存旧 compose、compose.env、hanabi.env、config、image ID 与 SQLite 一致备份（35,295,232 bytes）；凭据未复制到本地或输出。Hanabi 镜像切换结果见后续记录。

### Hanabi 切换结果（2026-10-09 00:04 北京时间）

- 已切换 `hanabi:quota-fix-20261008`；image ID `sha256:b5557b32bdb555dd64e6c29c852e7e31a231b1d1fa959b6895501be74260d5b5`，binary SHA-256 `f3f5cd41b04494cc4408cbb9218bafdb0524658611fd52f6832d4bdd4e272015`。目标原生 ARM64 静态构建，clean baseline 442cbb8 + 仅 gallery_sync.rs 差异，不含本地未提交 image_dedup/telegram/README。镜像runtime继承旧生产image，9项运行参数一致，只增加新binary层。
- 同一干净上下文在 Oracle 上 `cargo test --release --locked --test gallery_sync -j4` 8 passed，退出0。日志在 `/home/ubuntu/hanabi-quota-build-20261008/`，本地副本 `/tmp/hanabi-quota-build-20261008/`。运行器只使用新cursor协议，按服务器摘要跳过相同内容，PUT失败/不完整确认下次仍有资格重试，不引入本地已发送状态。
- `/var/lib/hanabi/config.toml` 的 `[gallery].fingerprint_sync_interval_secs = 86400`，其他配置语义逐项比较一致；compose `.env` 仅 HANABI_IMAGE 改为新本地镜像。原文件owner/mode保留。
- docker StartedAt `2026-10-08T16:04:31.41653167Z`，running、RestartCount=0。首次启动同步真实返回 D1 quota 503，正常结束后按一天间隔等待，没有继续高频请求；因此未声称生产全目录0PUT已完成。08:05 follow-up已授权可做一次受控重启验收并将每日同步对齐到早晨。
- 切换前后都核对了4条pending、7个文件，全存在且都有publication记录；另2条staging未确认，不处理。SQLite第二份切换前备份 `hanabi.pre-switch.db` 保存在上述备份目录。未新建频道消息、未代审批或删除作品。
- 回滚Hanabi：核对没有正在进行的发布后，把备份目录的 compose.env/config.toml 恢复到原位置（保留权限），在 `/opt/hanabi-container` 执行 `sudo docker compose up -d --no-deps hanabi`。旧image `sha256:7eb32fdf056a8f3498d7d53d6580bdf99d7c12c7b6d02988c941ceea035c038c` / v0.11.23仍保留。无需恢复数据库：新代码无本地schema变更。Worker兼容旧catalog协议；但若回滚Worker到旧协议，须同步回滚Hanabi。

## 2026-10-09 08:05 配额重置后收尾

- 已核对生产 Worker 仍为 `83c03134-06af-410c-b303-96dcf7d2ab75`，Hanabi 仍为 `b5557b32...`，没有覆盖期间其他部署。
- `0013_catalog_read_cost.sql` 已成功应用；远端 sqlite_master 确认两个部分索引和三个 work_tags 版本触发器均存在。迁移运行7条命令，并一次性失效旧缓存。
- 真实公开 API（Oracle）验证：tags 为 MISS→HIT，换搜索词仍 HIT；sources 为 MISS→HIT。Mac Python 的请求被 Cloudflare 403，而签入 Chrome 正常；未弱化 WAF。Chrome实际显示24张卡片，前6张640px缩略图完整加载。
- 使用与列表一致的实际 D1 SELECT 验证24作品读取152行，Pixiv来源24作品读取234行；单个generation读取1行，均0写入。不是用VM步数冒充D1计费行数。
- 发现本次待入库已增为5条/10文件；逐项核实publication与原文件后，只调整原gallery_outbox的next_attempt_at，让现有重试循环处理。没有调用频道发送。5条全部补入：x:2108166444232135049；douyin:7694171775450977707、7693543400276047218、7693920321389014897、7693914571413482314。D1确认五作品在线、page_count与images数量分别2/1/1/3/3，总计10。
- 队列现仅剩原两条未确认staging，未处理。重试前SQLite备份与精确目标清单保存在Oracle `/opt/hanabi-container/backups/20261009-quota-acceptance/`。
- 确认无uploading/pending及近期发送后，于08:13:20仅重启Hanabi一次；进程正常、RestartCount=0。08:13:56真实同步完成：listed=4325、imported=8、unchanged=4317、failed=0、pushed=10。原有4315张未全量重传；每日86400秒设置保持不变。
- 本地独立复核90组标签语义、目录完整导出与摘要黄金向量通过；索引计划采用范围查找且无临时排序。写入回归沿用上一轮隔离测试，本轮没有用生产作品测试隐藏/重复PUT。
- 恢复截图：`/Users/furina/.codex/visualizations/2026/10/08/01a1195c-aa47-7222-9e68-6f0117a4e7af/vitrine-restored-20261009.png`。08:15 Oracle相似任务验收结果见下一条记录。

- 08:15每日Oracle任务自动触发并成功结束（未手动重复扫描）：10个缺失CLIP全部 pushed=verified=10、failed=0、remaining=0；4325 fingerprints / 4325 CLIP，generation=1970 published。后续轻量snapshot确认 result_generation=1970、result_fresh=true。耗用29.381秒CPU、峰值1.0GiB、0swap，四核上限保留；下次触发2026-10-10 08:15:04。
- 当前图库已恢复浏览，来源总数2788；本次Cloudflare统计快照为452455 rowsRead / 5911 rowsWritten，包含迁移、补入库、缓存冷建、每日扫描与验收，不作为全天稳定负载预测。
- 本次收尾完成，一次性跟进 automation 将暂停；每日Oracle timer与Hanabi 86400秒同步继续。仍保留2条未确认staging，无新增审批、删除或频道发送。

## 2026-10-09 详情加载与过渡

- 已发布 Worker `7633f841-f011-4e2b-a226-770c6270be17`，只更新 `index.html`、`gallery.css`、`app.js` 三个静态资源，沿用上一版本构建的 Worker。未修改数据库、媒体权限、原图或扫描计划。
- 点开作品先复用已加载的列表封面，再在清晰预览解码完成后淡入；详情元数据返回不重复重置同一图片。按展示尺寸选择现有640/1280预览，取消前端预览失败后自动下载原图，保留手动查看原图和预览重试。
- 使用原生 Web Animations 给详情添加360ms展开、230ms收回，遮罩同步淡入淡出，图片区域固定高度避免载入跳动。关闭立即取消详情请求，动画结束才关闭dialog；快速重开、历史前进返回及减少动态效果均受保护。
- 新增 `tools/test_viewer.mjs` 并接入 `npm run check`，覆盖渐进预览、同URL复用、640尺寸、加载/解码失败、过期回调、关闭/重开/历史导航及reduced-motion。viewer、frontend、admin_ui、glass测试和JS语法/diff检查均通过；本轮未改后端，未重复整套后端回归。
- 正式Chrome实测：点开 `douyin:7662702600295344357` 时，1280预览仍在加载，640封面已加载且可见；之后清晰图ready。390px手机视口切图实际请求640，dialog/client/scroll宽度均390，无横向溢出。桌面1280显示、切图与动画关闭通过，浏览器无error/warn；本地补验Esc和前进返回。
- 网络只做小样本检查：一张X作品原图HEAD为717,798字节，640/1280实际返回WebP分别91,658/141,108字节；该样本未走后端原图回退。没有大规模预热/扫描；冷转换、其他地区网络与Safari真机速度不由该样本保证。
- 备份/发布日志：`/tmp/vitrine-detail-20261009/`。可用现有Wrangler回滚至 `83c03134-06af-410c-b303-96dcf7d2ab75`，无需数据库回滚。正式截图：`/Users/furina/.codex/visualizations/2026/10/08/01a1195c-aa47-7222-9e68-6f0117a4e7af/vitrine-detail-desktop-20261009.png` 与同目录 `vitrine-detail-mobile-20261009.png`。

## 2026-10-10 每日扫描后的审批连续性

- 已部署 Worker `6b334471-22f7-471d-9e57-d39e46615b0d`，前版 `7633f841-f011-4e2b-a226-770c6270be17`。发布更新后端与 `admin.html`、`admin.js`，没有数据库迁移、生产审批/删除或 Oracle 调度修改。
- 原因：今日 Oracle 08:15:08 开始、08:20:22 成功发布 generation=3014，补扫200张，fingerprints/CLIP均4525；随后新增图片令当前generation=3021，旧版要求两者完全相等，因而隐藏整批候选。D1只读快照证实缓存仍有97组和11对，读取2行、写入0行。
- 目录变化后保留上次候选范围，仅通过候选r2_key索引读取当前在线指纹和CLIP，重新核对组成员、所属作品、相似关系和保留项；不能验证的组暂不显示。严格去重对仍须满足当前严格匹配与原保留项质量条件。新候选发现继续由Oracle每日扫描完成；未手动重跑全库。
- 复核结果使用现有Cache API，以当前generation和原扫描payload摘要隔离，前后读取D1版本确保一致。算法版本检查、跳过组过滤、隐藏/删除成员过滤保留。30,000行隔离SQLite证明候选查询使用键索引：两键149 VM步；这不是D1计费行数。
- 删除事务新增内部generation guard，阻止复核后并发特征修改；客户端不能提供该参数。同批后续项目只沿用前一删除事务内返回的post-generation，避免第一张删除后误阻断整批，也不盲目接受外部更新。原回执重放、撤销、作品/频道映射快照、所有重叠组保留项保护保持有效。
- 后台显示北京时间最近扫描时间、每日08:15更新及新增待扫描说明。已核验旧候选可继续审批，自动5分钟刷新保留勾选，后台不可见或日额度耗尽时仍停止刷新。
- 验证：70项Rust测试通过（1项历史性能测量忽略）、Clippy/fmt、所有前端套件、六组隔离HTTP集成通过。新增回归覆盖无全库重发情况下旧strict/CLIP审批、同批删两张留一张、特征变化拒绝、隐藏成员、重放不推进generation，保留并发/撤销回归。所有写入测试只用本地临时D1/R2，无Telegram请求。
- 正式Chrome实际恢复69组相似候选和11对去重建议（显示结果仍应用既有跳过记录），3张新增图片等待下轮。刷新后仍可见，首组四张图片完整载入，浏览器无error/warn。没有用真实作品测试删除。
- 回滚：现有Wrangler回滚至 `7633f841-f011-4e2b-a226-770c6270be17` 即可，无数据恢复。源码/旧构建、回归、发布证据在 `/tmp/vitrine-review-20261010/`。正式截图：`/Users/furina/.codex/visualizations/2026/10/08/01a1195c-aa47-7222-9e68-6f0117a4e7af/vitrine-similar-restored-20261010.png`。

### 相似图来源标记

- 发布 `47380be0-a075-4bb5-b013-648a961e29c0`：每张相似图左上角增加来源标签，按接口source显示X、Pixiv、抖音；缺失来源显示未知来源。沿用现有主题色，标签不接收点击，保留勾选/原图/审批行为。
- 仅更新admin.js、styles.css、admin.html静态资源。来源映射及原有后台/前台回归通过；后端构建与前版保持一致，没有作品数据或扫描变更。
- 备份与部署日志 `/tmp/vitrine-source-badges-20261010/`，回滚版本 `6b334471-22f7-471d-9e57-d39e46615b0d`。
- Chrome正式页确认167张候选卡片均有来源标签，实际包含X、Pixiv、抖音三种；截图 `/Users/furina/.codex/visualizations/2026/10/08/01a1195c-aa47-7222-9e68-6f0117a4e7af/vitrine-similar-sources-20261010.png`。本轮只改显示，未重复后端或生产删除测试。

## 2026-10-10 空白图过滤与手动勾选

- Worker已发布 `b1519503-aef4-4df5-8221-8e6967ee8f59`（前版 `47380be0-a075-4bb5-b013-648a961e29c0`）。相似审批卡片全部默认不勾选，自动刷新只保留本页手动选择，手动刷新重新清空；来源标签和至少保留一张的服务端保护不变。
- 修复CLIP分组没有检查solid_color的问题。当前与过期扫描缓存都执行候选资格核验，排除solid_color；复核Cache API命名空间升至v2，不能沿用旧缓存回显。Oracle原生分组器同步修复，仍每日08:15、四核，不手动启动全库扫描。
- 两张真实原图分别为 `douyin/7681127579873278306/d69ac7240da93090b0a4d6bd9bbc5a38/01.jpg`、`douyin/7679918512127666420/e81dcfc5b8aeba4824c52d50cb98551d/01.jpg`，只下载198560与143756字节，SHA与D1指纹一致。它们并非严格数学纯色：第一张maxRGB均值0.778425、99.9分位11，第二张均值0.001503、99.9分位0；少量亮点使原先全图通道范围<=5规则保留了它们。
- Hanabi新增保守的近黑填充页分支：至少256×256、完全不透明、maxRGB均值<=1且至少99.95%像素maxRGB<=16；纯色/全透明原规则保留，小图和带alpha形状不走新分支。两张真实原图用共享Rust判定均为solid_color=true；边界、小图单点细节、暗色层次和透明形状测试通过。未扩展近白过滤，未重扫其他历史原图。
- Hanabi部署 `hanabi:solid-filter-20261010`，image ID `sha256:9a20ea24bae0d8424596e54a5fbc25e694303a4a5f5b607f7a3d9362bc195313`，binary SHA `cbc176ac9ba522f7695747f0ceda23b45f42adecb36cb87adcec6e3d907a72d4`。从已验证quota-fix干净上下文构建，运行源码仅image_dedup.rs改变，未混入主工作树其他严格去重/并行指纹改动。镜像继承原运行环境，9项镜像配置一致，容器环境按键值核对一致（顺序变化）、挂载与配置保持，86400秒指纹同步保留。
- Oracle ARM64原生18项图片/同步测试通过，隔离基线全套141项测试与Clippy/fmt通过；Hanabi本地主树合并后图片/同步测试通过。Vitrine 71项Rust测试通过（1项历史性能测量忽略）、前端与六组隔离HTTP回归通过，专门覆盖CLIP相同向量和generation一致的旧缓存也不能返回纯色图。
- 仅将上述两张图的Hanabi历史指纹和Vitrine指纹solid_color改为true，原始指纹已备份，照片SHA核对一致。没有删除作品/原图、代审批或重发频道消息。Hanabi切换前158条pending均待审批、无claimed图库任务；19:45:23启动后running、RestartCount=0，正常接收新待审项。真实启动同步listed=4525、imported=8、unchanged=4517、failed=0、pushed=20，未全量PUT，两张修正标记同步后仍为true。
- Oracle分组器current已切至 `/home/ubuntu/vitrine-similar/releases/20261010-solid-filter`，binary SHA `7886c7bd74b3cfa27d4a6bd0dcecf0c2243f3eda7d8def1510b40c0084f60a29`；在原生二进制上输入两张solid且CLIP相同的离线fixture返回空groups/auto_pairs，未调用生产扫描。
- Chrome正式页确认22张当时可见候选全部未勾选，投诉的两张图不再出现；期间用户自行审批导致候选总数变化，不将其归因于本次过滤。截图 `/Users/furina/.codex/visualizations/2026/10/08/01a1195c-aa47-7222-9e68-6f0117a4e7af/vitrine-manual-selection-20261010.png`。
- 回滚与证据：本地 `/tmp/vitrine-solids-20261010/`；Oracle `/opt/hanabi-container/backups/solid-filter-20261010/` 保留切换前SQLite、两条原指纹、compose/env/config、旧容器状态及启动验收。旧镜像 `hanabi:quota-fix-20261008` 仍保留，可仅恢复HANABI_IMAGE并compose up，无需恢复整库。分组器旧current `/home/ubuntu/vitrine-similar/releases/20261008-d1-reads`，指针和新旧SHA保存在 `rollback/20261010-before-solid/`。空白标记可按精确两条备份单独恢复，不覆盖其后正常入库数据。

## 2026-10-11 浏览体验与加载开销

- 调整栏目标题、收藏计数、来源选中态和卡片悬停反馈；来源筛选集中到导航，热门标签可横向滚动。首屏和筛选切换有八张无障碍隐藏的骨架占位，分页加载反馈留在列表底部。
- 首四张封面立即加载，其余仅在视口前后 360px 范围请求；不支持 IntersectionObserver 时保留原生 lazy。重置时移除观察，陈旧回调跳过已移除图片。缩图失败显示详情入口，不自动下载原图；详情保留预览重试与显式查看原图。
- 来源、标签各自完成后呈现，保留输入草稿。WebGL 仅在已加载图像进入玻璃采样区时按表面初始化，保持事件驱动绘制和减少透明度降级。
- `npm run check` 通过：71 项 Rust 测试、1 项既有忽略项，Clippy、前端/查看器/玻璃/后台回归与 WASM 构建正常。补充回归覆盖图片延迟加载、无 IO 降级、失败不取原图、过期观察通知、骨架清理、独立元数据渲染。release 构建也通过。
- Chrome 正式站 1440×1000 同配置单次 trace：CLS 0.1711 → 约 0，LCP 1262ms → 1122ms；初始封面请求 20 → 12，首屏 WebGL canvas 4 → 0。LCP 是本机网络/缓存条件下样本，不代表真实用户分布或固定提升。
- 本地 320/390 与桌面无横向溢出；正式站 390 验证多图第 2 页及分享页码、来源筛选、100 个标签索引、无结果/清除、自动分页 24 → 48 且无重复、深浅主题和回顶。正式只读检查通过，包括三档真实 WebP 宽度、ETag、公开接口权限与旧缩图入口拦截。既有 Cloudflare beacon 被 CSP 阻止的控制台消息仍存在，没有放宽 CSP。
- 发布版本 `5f7f49ef-30d5-4f9a-b7e7-54de1de93629`。本地重新构建的后端与线上不同，因此此次复用下载的线上 JS/WASM，只上传 index.html、gallery.css、glass.js、app.js、admin.html 五个资产；发布后两份模块 SHA-256 和绑定/运行时设置与发布前一致。没有数据库迁移、媒体写入或后台业务变更。
- 回滚：`npx wrangler rollback cff623b3-76a0-4727-a3ac-5b438d8b3ec4`。备份、模块校验、构建和发布日志在 `/tmp/vitrine-optimize-20261011/`。未做 iPhone 真机或本轮 Safari 验收，也未将本地缓存测试当作线上速度。

## 2026-10-11 v0.2.0 源码收尾

- 首个 GitHub 源码版本定为 v0.2.0，统一 Cargo/npm 版本。公开图库、管理后台、可恢复审核、Oracle 分组器与迁移 0007–0013 纳入同一源码快照；不发布 npm/crates.io 包。GitHub Release 不触发生产部署。
- 核清前次保留的线上后端：下载的 WASM 为 1,376,991 字节、SHA-256 `f01588d4a05af849b466785b6b27fac714df335b0da541a4043ebc8e5bc57de7`，仍包含 multipart/hash/upload 三条临时计时日志。临时向本地源码恢复这三条日志后，release 构建与线上 WASM 逐字节完全相等；再恢复源码，保留无计时日志版本。因此已确认业务 Rust 源码一致，不能把旧记录中的“已移除线上计时日志”当作事实。本轮未覆盖线上 JS/WASM。
- README 同步迁移范围、旧候选复核行为、原生流式入库及免费 Worker 大上传 CPU 限制；`.gitignore` 覆盖所有 `.dev.vars*`。Oracle 的扫描器、运行器和 SQLite 读成本离线检查接入 `npm run check`，避免后续只检查 Worker 而漏掉任务脚本。
- 本轮 `CARGO_BUILD_JOBS=2 npm run check`、release WASM 构建通过：71 项 Rust 测试通过，1 项既有性能测量忽略；前端/查看器/玻璃/后台 UI 回归通过；Python 扫描器 14 项、运行器 17 项及 30,000 行 SQLite 游标/候选/generation 检查通过。
- 七组独立临时 D1/R2 HTTP 回归通过，包含登录/CSRF/请求体限制、隐藏恢复、重新入库、并发删除/撤销、过期候选、安全回执、目录 `catalog_id` / `next_cursor` / `fingerprint_digest` 协议与相同 PUT 不推进版本，以及 96 MiB 入库原图字节/SHA-256/幂等重放。测试不加载项目凭据，不访问生产 D1/R2 或 Telegram。
- 正式站只读验收通过：安全头、catalog 未授权拒绝、游标与标签索引、WebP 160/640/1280 实际宽度、ETag 304 和旧裸缩图入口 403；三个缩图合计 270,968 字节。未用生产作品执行编辑/删除/恢复或大上传。
- 本轮 `npm audit` 报告 0 条漏洞；检查仅覆盖 npm 锁定开发依赖，不保证整个系统无漏洞。
- 未包含未被页面引用的 `public/water.js`、本地 `.agents/` 技能安装、`skills-lock.json`、`.c2cignore`、旧设计草稿、临时日志、数据库备份、构建产物或密钥。未执行远端迁移、部署、Oracle 更新或 Telegram 操作；iPhone 真机与本轮 Safari 尚未复验，免费 Worker 1102 风险仍须单独处理。

## 2026-10-11 v0.2.0 正式部署

- 用户明确授权部署后，将源码提交 `8bee5b6968e67fdac87ffa7cebb04487047de616` 发布至 Worker `vitrine` / `gallery.fontaine.blue`，新版本 `69de2943-9efd-4958-ad94-4d7c6251ddef` 承接 100% 流量。静态资产仅取该提交的 `public/`，未包含本地未跟踪文件；Wrangler 4.148.0 使用 `--keep-vars` 和现有正式配置。四个资产因去除尾部空白而更新，页面业务代码保持原样。
- 发布前重新下载线上模块、settings 与 deployment，并核对当前版本仍为 `5f7f49ef-30d5-4f9a-b7e7-54de1de93629`。备份位于 `/tmp/vitrine-deploy-closeout-20261011/before/`（受限权限），同级 `rollback.sh` 可回滚至该版本；旧版本仍保留，不需恢复数据库。
- 发布后重新下载的 WASM 为 1,376,163 字节，SHA-256 `e0806bc12c2803f358bdd56df549eacc79e2237ede36c428df5b87742b277abb`，与 v0.2.0 本地 release 构建逐字节相同；确认不含 `ingest timing:`。JS 模块 SHA-256 `56ac17ff8f2ff6448e1c1c19a6f1d50fb27a7f70004bd771baf4716b06e0d0a8`。前后 settings 仅部署 annotations 变化，全部 bindings（包括密钥身份）、变量、兼容日期/flags、usage_model、placement、tail/logpush 均一致。workers.dev / previews 仍关闭。
- 远端迁移列表确认无待应用迁移；没有创建或迁移 D1/R2，没有业务数据写入，也没有 Telegram 操作。正式只读套件通过，三档 WebP 实际宽度 160/640/1280、总 270,968 字节，ETag 304、游标、标签索引、安全头和旧裸缩图入口 403 正常。七个公开资产逐字节匹配提交；匿名 catalog/snapshot 为 401，后台入口保留 WAF 403。
- Oracle 上实际 Bearer 调用 health、两项 catalog 增量页与 similarity snapshot 成功，`catalog_id`、非空 `fingerprint_digest` 与 `next_cursor` 齐全。验收快照 generation=3975、result_generation=3807，后续入库使结果过期属既有每日扫描安排；未为验收强制全库扫描，旧候选仍需按当前成员/特征复核。
- Oracle current 仍为 `20261010-solid-filter`。运行中的两份 Python 脚本 SHA 与源码一致，服务源码目录中的 `similar.rs` / CLI 源码 SHA 与发布提交一致；现有 ARM64 二进制 SHA `7886c7bd74b3cfa27d4a6bd0dcecf0c2243f3eda7d8def1510b40c0084f60a29`。今日任务成功补齐142张CLIP并发布generation=3807，四核/2GiB限制、每日08:15 timer均保留。因此未重建、重启或修改Oracle任务。
- 回滚、dry-run、发布、模块/settings、资产权限和公开API验收证据保存在 `/tmp/vitrine-deploy-closeout-20261011/`。免费 Worker 大上传1102与iPhone/本轮Safari真机路径仍未完成验证；未用生产作品做上传、删除或恢复测试。

## 2026-10-23 现状更正

- 后台不再有 WAF Challenge。zone `fontaine.blue` 的自定义规则只剩一条：拦截 `gallery.fontaine.blue/cdn-cgi/image/*`，确保缩图都经过 Worker 的可见性校验。后台靠 `INGEST_TOKEN` 登录、每分钟 5 次的登录限速、签名 HttpOnly cookie 和 CSRF 校验保护；`/api/admin/*` 未登录返回 401。
- `vitrine-media` 绑定了公开自定义域 `img.furina.ai`，供前端「复制图床链接」和 log.furina.ai 的 `bench/` 测评图使用。这条路径直接读 R2，不经过 D1 可见性校验：知道 key 的人仍能取到已下线作品的原图。
- 同日清理 R2：删除 D1 未引用的旧媒体对象 1,131 个、`review-trash/` 撤销备份 361 个和 `healthcheck/` 测试对象 1 个，共约 1.61 GB；`orphan_objects` 中 483 条待清理记录标为已删除。保留在线图片 5,828 个（7.05 GB）和 `bench/` 103 个。此前删除的单图和相似图决策从此无法撤销；后台撤销本就只在删除时的浏览器会话内可用。
- 同日按用户确认移除 R18：迁移 `0023` 删除 `works.is_r18`，入库忽略 Hanabi 传来的该字段（Worker `1476bddb-d2eb-4f56-8802-11c727207b69`）。

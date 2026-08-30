# Vitrine · Cloudflare 自托管图库

配合 [hanabi](https://github.com/Furinelle/hanabi) 使用的二次元图库：

- **存储**：图片进 R2，元数据/标签进 D1  
- **入库**：`POST /api/ingest`（Bearer token + multipart）  
- **浏览**：Workers 静态页，按 **来源** / **标签** / 关键词筛选  
- **标签**：使用帖子自带 tags（Pixiv/X），不做 AI 打标  

## 部署

```bash
cd vitrine
npm install

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

入库限制：最多 40 个文件、单文件 50 MiB、单次请求总计 100 MiB。

### `GET /api/works?source=&tag=&q=&limit=&offset=`

### `GET /api/tags` · `GET /api/sources`

### `GET /media/<r2_key>`

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
npm install
# 需已安装 Rust + wasm32-unknown-unknown + worker-build
# rustup target add wasm32-unknown-unknown
# cargo install worker-build

npm run db:local    # apply D1 migrations 0001–0004 to a local database only
npm run build:dev   # worker-build --dev → build/worker/shim.mjs
npx wrangler dev

# 质量检查（不部署）
npm run check       # fmt + clippy -D warnings + cargo test + worker-build --dev
```

`npm run db:local` is the only migration command used during development. It creates `telegram_publications` and `catalog_work_prune_receipts` from `migrations/0004_telegram_publications.sql`.

`npm run db:remote` is operator-only. Do not run it from an agent session. Remote migration 0004 requires a D1 export/backup and explicit operator approval before applying.

# Shirogane · Cloudflare 自托管图库

配合 [hanabi](https://github.com/Furinelle/hanabi) 使用的二次元图库：

- **存储**：图片进 R2，元数据/标签进 D1  
- **入库**：`POST /api/ingest`（Bearer token + multipart）  
- **浏览**：Workers 静态页，按 **来源** / **标签** / 关键词筛选  
- **标签**：使用帖子自带 tags（Pixiv/X），不做 AI 打标  

## 部署

```bash
cd shirogane
npm install

# 1) 创建 D1 + R2（账号内执行一次）
npx wrangler d1 create shirogane
npx wrangler r2 bucket create shirogane-media

# 2) 把输出的 database_id 填进 wrangler.jsonc 的 d1_databases[0].database_id

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

```bash
npm run db:local
npx wrangler dev
```

import type { Env, IngestMeta, WorkRow } from "./types";

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url);
    const path = url.pathname;

    try {
      if (request.method === "OPTIONS") {
        return cors(new Response(null, { status: 204 }));
      }

      if (path === "/api/health") {
        return json({ ok: true, service: "vitrine" });
      }

      if (path === "/api/ingest" && request.method === "POST") {
        return cors(await handleIngest(request, env));
      }

      if (path === "/api/works" && request.method === "GET") {
        return cors(await handleListWorks(url, env));
      }

      if (path === "/api/tags" && request.method === "GET") {
        return cors(await handleListTags(env));
      }

      if (path === "/api/sources" && request.method === "GET") {
        return cors(await handleListSources(env));
      }

      if (path.startsWith("/media/") && request.method === "GET") {
        return handleMedia(path.slice("/media/".length), env);
      }

      // SPA / static
      if (env.ASSETS) {
        return env.ASSETS.fetch(request);
      }
      return new Response("vitrine online", { status: 200 });
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      console.error("vitrine error", message);
      return cors(json({ ok: false, error: message }, 500));
    }
  },
};

function cors(res: Response): Response {
  const headers = new Headers(res.headers);
  headers.set("Access-Control-Allow-Origin", "*");
  headers.set("Access-Control-Allow-Methods", "GET, POST, OPTIONS");
  headers.set("Access-Control-Allow-Headers", "Authorization, Content-Type");
  return new Response(res.body, { status: res.status, headers });
}

function json(data: unknown, status = 200): Response {
  return new Response(JSON.stringify(data), {
    status,
    headers: { "Content-Type": "application/json; charset=utf-8" },
  });
}

function requireAuth(request: Request, env: Env): Response | null {
  const token = env.INGEST_TOKEN;
  if (!token) {
    return json({ ok: false, error: "INGEST_TOKEN not configured" }, 500);
  }
  const auth = request.headers.get("Authorization") || "";
  const expected = `Bearer ${token}`;
  if (auth !== expected) {
    return json({ ok: false, error: "unauthorized" }, 401);
  }
  return null;
}

function normalizeTag(raw: string): string {
  return raw.trim().replace(/^#/, "").replace(/\s+/g, "_").slice(0, 64);
}

function workId(source: string, sourceId: string): string {
  return `${source}:${sourceId}`;
}

function guessContentType(name: string): string {
  const lower = name.toLowerCase();
  if (lower.endsWith(".png")) return "image/png";
  if (lower.endsWith(".webp")) return "image/webp";
  if (lower.endsWith(".gif")) return "image/gif";
  if (lower.endsWith(".bmp")) return "image/bmp";
  return "image/jpeg";
}

async function handleIngest(request: Request, env: Env): Promise<Response> {
  const denied = requireAuth(request, env);
  if (denied) return denied;

  const contentType = request.headers.get("Content-Type") || "";
  if (!contentType.includes("multipart/form-data")) {
    return json({ ok: false, error: "expected multipart/form-data" }, 400);
  }

  const form = await request.formData();
  const metaRaw = form.get("meta");
  if (typeof metaRaw !== "string") {
    return json({ ok: false, error: "missing meta field" }, 400);
  }

  let meta: IngestMeta;
  try {
    meta = JSON.parse(metaRaw) as IngestMeta;
  } catch {
    return json({ ok: false, error: "invalid meta json" }, 400);
  }

  const source = String(meta.source || "").trim().toLowerCase();
  const sourceId = String(meta.source_id || "").trim();
  if (!source || !sourceId) {
    return json({ ok: false, error: "source and source_id required" }, 400);
  }

  const files = form
    .getAll("files")
    .filter((v): v is File => typeof v !== "string" && v !== null);

  if (files.length === 0) {
    return json({ ok: false, error: "no files" }, 400);
  }

  const id = workId(source, sourceId);
  const now = new Date().toISOString();
  const tags = (meta.tags || [])
    .map(normalizeTag)
    .filter(Boolean)
    .filter((t, i, arr) => arr.indexOf(t) === i)
    .slice(0, 40);

  // upsert work
  await env.DB.prepare(
    `INSERT INTO works (id, source, source_id, source_url, title, author_name, author_url, is_r18, page_count, origin, created_at)
     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
     ON CONFLICT(id) DO UPDATE SET
       source_url=excluded.source_url,
       title=excluded.title,
       author_name=excluded.author_name,
       author_url=excluded.author_url,
       is_r18=excluded.is_r18,
       page_count=excluded.page_count,
       origin=excluded.origin`
  )
    .bind(
      id,
      source,
      sourceId,
      meta.source_url || "",
      meta.title || "",
      meta.author_name || "",
      meta.author_url || "",
      meta.is_r18 ? 1 : 0,
      files.length,
      meta.origin || "",
      now
    )
    .run();

  // replace tags
  await env.DB.prepare(`DELETE FROM work_tags WHERE work_id = ?`).bind(id).run();
  for (const tag of tags) {
    await env.DB.prepare(
      `INSERT INTO tags (name, use_count) VALUES (?, 1)
       ON CONFLICT(name) DO UPDATE SET use_count = use_count + 1`
    )
      .bind(tag)
      .run();
    await env.DB.prepare(
      `INSERT OR IGNORE INTO work_tags (work_id, tag) VALUES (?, ?)`
    )
      .bind(id, tag)
      .run();
  }

  // remove old images for re-ingest
  const oldImages = await env.DB.prepare(
    `SELECT r2_key FROM images WHERE work_id = ?`
  )
    .bind(id)
    .all<{ r2_key: string }>();
  for (const row of oldImages.results || []) {
    try {
      await env.MEDIA.delete(row.r2_key);
    } catch {
      /* ignore */
    }
  }
  await env.DB.prepare(`DELETE FROM images WHERE work_id = ?`).bind(id).run();

  const saved: Array<{ page: number; r2_key: string; bytes: number }> = [];
  for (let i = 0; i < files.length; i++) {
    const file = files[i];
    const buf = await file.arrayBuffer();
    const ct = file.type || guessContentType(file.name || `p${i}.jpg`);
    const ext = ct.includes("png")
      ? "png"
      : ct.includes("webp")
        ? "webp"
        : ct.includes("gif")
          ? "gif"
          : "jpg";
    const key = `${source}/${sourceId}/${String(i).padStart(2, "0")}.${ext}`;
    await env.MEDIA.put(key, buf, {
      httpMetadata: { contentType: ct },
      customMetadata: {
        work_id: id,
        page_index: String(i),
      },
    });
    const imageId = `${id}#${i}`;
    await env.DB.prepare(
      `INSERT INTO images (id, work_id, page_index, r2_key, content_type, byte_size, created_at)
       VALUES (?, ?, ?, ?, ?, ?, ?)`
    )
      .bind(imageId, id, i, key, ct, buf.byteLength, now)
      .run();
    saved.push({ page: i, r2_key: key, bytes: buf.byteLength });
  }

  return json({
    ok: true,
    work_id: id,
    pages: saved.length,
    tags,
    images: saved,
  });
}

async function handleListWorks(url: URL, env: Env): Promise<Response> {
  const tag = (url.searchParams.get("tag") || "").trim();
  const source = (url.searchParams.get("source") || "").trim().toLowerCase();
  const q = (url.searchParams.get("q") || "").trim();
  const limit = Math.min(Math.max(Number(url.searchParams.get("limit") || 40), 1), 100);
  const offset = Math.max(Number(url.searchParams.get("offset") || 0), 0);

  const where: string[] = [];
  const binds: unknown[] = [];

  if (source) {
    where.push("w.source = ?");
    binds.push(source);
  }
  if (tag) {
    where.push(
      "EXISTS (SELECT 1 FROM work_tags wt WHERE wt.work_id = w.id AND wt.tag = ?)"
    );
    binds.push(normalizeTag(tag));
  }
  if (q) {
    where.push("(w.title LIKE ? OR w.author_name LIKE ? OR w.source_id LIKE ?)");
    const like = `%${q}%`;
    binds.push(like, like, like);
  }

  const whereSql = where.length ? `WHERE ${where.join(" AND ")}` : "";
  const sql = `
    SELECT w.*,
      (SELECT i.r2_key FROM images i WHERE i.work_id = w.id ORDER BY i.page_index LIMIT 1) AS cover_key,
      (SELECT GROUP_CONCAT(wt.tag, ',') FROM work_tags wt WHERE wt.work_id = w.id) AS tags
    FROM works w
    ${whereSql}
    ORDER BY w.created_at DESC
    LIMIT ? OFFSET ?
  `;
  binds.push(limit, offset);

  const stmt = env.DB.prepare(sql).bind(...binds);
  const result = await stmt.all<WorkRow>();
  const works = (result.results || []).map((row) => ({
    id: row.id,
    source: row.source,
    source_id: row.source_id,
    source_url: row.source_url,
    title: row.title,
    author_name: row.author_name,
    author_url: row.author_url,
    is_r18: !!row.is_r18,
    page_count: row.page_count,
    origin: row.origin,
    created_at: row.created_at,
    cover_url: row.cover_key ? `/media/${row.cover_key}` : null,
    tags: row.tags ? String(row.tags).split(",").filter(Boolean) : [],
  }));

  return json({ ok: true, works, limit, offset });
}

async function handleListTags(env: Env): Promise<Response> {
  const result = await env.DB.prepare(
    `SELECT t.name, COUNT(wt.work_id) AS cnt
     FROM tags t
     LEFT JOIN work_tags wt ON wt.tag = t.name
     GROUP BY t.name
     HAVING cnt > 0
     ORDER BY cnt DESC, t.name ASC
     LIMIT 200`
  ).all<{ name: string; cnt: number }>();
  return json({ ok: true, tags: result.results || [] });
}

async function handleListSources(env: Env): Promise<Response> {
  const result = await env.DB.prepare(
    `SELECT source, COUNT(*) AS cnt FROM works GROUP BY source ORDER BY cnt DESC`
  ).all<{ source: string; cnt: number }>();
  return json({ ok: true, sources: result.results || [] });
}

async function handleMedia(key: string, env: Env): Promise<Response> {
  const decoded = decodeURIComponent(key);
  const obj = await env.MEDIA.get(decoded);
  if (!obj) {
    return new Response("not found", { status: 404 });
  }
  const headers = new Headers();
  obj.writeHttpMetadata(headers);
  headers.set("etag", obj.httpEtag);
  headers.set("Cache-Control", "public, max-age=31536000, immutable");
  return new Response(obj.body, { headers });
}

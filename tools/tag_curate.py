"""Scheduled tag curation for new works: collect unsorted tags, have Grok research them, apply the
decisions Claude signs off on.

    python3 tools/tag_curate.py collect            # new run folder with candidates.json and library.json
    python3 tools/tag_curate.py research RUN       # Grok searches each tag -> RUN/proposals.json
    python3 tools/tag_curate.py apply RUN [--dry-run]
                                                   # RUN/decisions.json (reviewed) -> backup, rehearsal, D1
    python3 tools/tag_curate.py drops              # drops waiting for the owner's review
    python3 tools/tag_curate.py approve-drops TAG... | --all
    python3 tools/tag_curate.py reject-drops TAG...

decisions.json is a list of {"tag", "action", ...}:
  merge  {"target"}                       the tag is another spelling of an existing tag
  item   {"name", "kind", "parent", "en", "ja", "members"}
                                          file it in the library (a different "name" renames it first)
  drop   {"note"}                         remove it from every work and block it (only tags on <= 2 works). A
                                          Japanese tag (it has kana) goes at once; any other waits for the
                                          owner: `drops` lists them, `approve-drops` / `reject-drops` settle them
  keep   {"note"}                         leave it unsorted; it is not proposed again for 30 days
Every decision is checked again before anything is written: unknown or blocked targets, missing parents
or members, implication cycles, display names that already belong to another tag and drops of tags in
use are refused and reported. The SQL is the admin tag page's (src/tag_admin.rs) written out, rehearsed
on a fresh export of the database, and only then run on D1.
"""
import datetime
import json
import pathlib
import sqlite3
import subprocess
import sys
import urllib.parse

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import tag_merge  # noqa: E402

ROOT = tag_merge.ROOT
STATE = pathlib.Path.home() / "Library/Application Support/vitrine-curate"
KINDS = {"work", "character", "skin", "cp", "lore", "theme", "medium", "meta"}
NO_ENGLISH = {"theme", "medium", "meta"}  # the owner's rule: themes and the like get no English name
DROP_MAX = 2
BATCH = 150
QUIET_DAYS = 30
WORK_SAMPLES = 3
GALLERY = "https://gallery.fontaine.blue"
GROK = ["grok", "-m", "grok-4.7-build-fast", "--effort", "xhigh", "--always-approve"]
sql_text = tag_merge.sql_text


def d1(sql):
    out = subprocess.run(["wrangler", "d1", "execute", "vitrine", "--remote", "--json", "--command", sql],
                         cwd=ROOT, capture_output=True, text=True, check=True).stdout
    return json.loads(out)[0]["results"]


def in_list(names):
    return "(" + ",".join(sql_text(n) for n in names) + ")"


def stored(name, empty):
    path = STATE / name
    return json.loads(path.read_text()) if path.exists() else empty


def store(name, value):
    STATE.mkdir(parents=True, exist_ok=True)
    (STATE / name).write_text(json.dumps(value, ensure_ascii=False, indent=1))


def seen():
    return stored("seen.json", {})


def japanese(tag):
    """Kana mark a tag as Japanese; kanji alone cannot tell it from Chinese."""
    return any("\u3040" <= c <= "\u30ff" or "\u31f0" <= c <= "\u31ff" or "\uff66" <= c <= "\uff9f" for c in tag)


def collect():
    today = datetime.date.today()
    quiet = {t for t, day in seen().items() if (today - datetime.date.fromisoformat(day)).days < QUIET_DAYS}
    quiet |= set(stored("pending_drops.json", {}))  # waiting for the owner already
    rows = d1("SELECT c.tag AS name, c.cnt FROM tag_counts c LEFT JOIN tag_items i ON i.name = c.tag "
              "WHERE c.cnt > 0 AND i.name IS NULL ORDER BY c.cnt DESC, c.tag")
    rows = [r for r in rows if r["name"] not in quiet][:BATCH]
    run = STATE / "runs" / datetime.datetime.now().strftime("%Y%m%dT%H%M%S")
    run.mkdir(parents=True)
    co, works = {}, {}
    names = [r["name"] for r in rows]
    for start in range(0, len(names), 40):
        batch = in_list(names[start:start + 40])
        for r in d1("SELECT wt.tag AS tag, wt2.tag AS co, COUNT(*) AS n FROM work_tags wt "
                    "JOIN work_tags wt2 ON wt2.work_id = wt.work_id AND wt2.tag <> wt.tag "
                    f"WHERE wt.tag IN {batch} GROUP BY wt.tag, wt2.tag"):
            co.setdefault(r["tag"], []).append((r["n"], r["co"]))
        # The works themselves explain a tag best: titles are often the whole post text, and the original post
        # (source_url) carries the artist's description.
        for r in d1("SELECT wt.tag AS tag, w.id, w.title, w.author_name, w.source, w.source_url FROM work_tags wt "
                    f"JOIN works w ON w.id = wt.work_id WHERE w.deleted_at IS NULL AND wt.tag IN {batch} ORDER BY w.created_at DESC"):
            listed = works.setdefault(r["tag"], [])
            if len(listed) < WORK_SAMPLES:
                listed.append({"title": r["title"][:400], "author": r["author_name"], "source": r["source"],
                               "source_url": r["source_url"], "gallery": f"{GALLERY}/?work={urllib.parse.quote(r['id'], safe='')}"})
    candidates = [{"tag": r["name"], "uses": r["cnt"],
                   "co_tags": {c: n for n, c in sorted(co.get(r["name"], []), reverse=True)[:15]},
                   "works": works.get(r["name"], [])} for r in rows]
    library = {r["name"]: {k: r[k] for k in ("kind", "parent", "ja", "en") if r[k]}
               for r in d1("SELECT name, kind, parent, ja, en FROM tag_items")}
    (run / "candidates.json").write_text(json.dumps(candidates, ensure_ascii=False, indent=1))
    (run / "library.json").write_text(json.dumps(library, ensure_ascii=False))
    print(json.dumps({"run": str(run), "candidates": len(candidates)}, ensure_ascii=False))


PROMPT = """你在帮一个二次元插画图库整理新标签。candidates.json 是还没归类的标签，每个带使用次数 uses、同作品上一起出现的其他标签 co_tags（用来判断出自哪个游戏），以及最多 3 件用到它的作品 works：title 往往就是原帖正文（含话题标签和作者的说明），author 是作者，source_url 是原帖，gallery 是图库里的作品页。判断标签含义时先看这些作品的标题和作者说明；拿不准时打开 source_url 看原帖的说明文字。library.json 是已有的标签库：键是存储用的名字（多数是中文），kind 取值 work/character/skin/cp/lore/theme/medium/meta，parent 是所属作品或角色。

逐个联网搜索每个标签的含义（游戏角色、地名、组织、版本名、活动名、皮肤名、英文名/日文名/缩写、画师或 VTuber 名等）。重点游戏：原神、崩坏星穹铁道、鸣潮、绝区零、崩坏3、明日方舟、明日方舟终末地、蔚蓝档案。很多是最新内容，必须搜索确认，不要凭记忆猜。

每个标签给一种处理（字段名照抄）：
- merge：它是 library.json 里某个条目或另一个已存在标签的其他写法（英文名、日文名、缩写、繁体、昵称），给 "target"（存储名）。
- item：独立的新条目，给 "name"（官方简体中文名；标签本身就是正式名时同名；日本作品角色可用日文原名），"kind"，"parent"（所属作品用 library.json 里的作品键，角色称号/皮肤的 parent 用角色键；必须是 library.json 的键或本批另一个 item 的 name），可选 "en"（只有常见、一眼能认出的官方英文名，首字母大写；题材、媒介、元信息不要英文名），可选 "ja"，CP 用 "kind": "cp" 和 "members"（角色名列表，必须已存在）。版本名、活动名、地名、组织用 lore。
- drop：画师/作者名、VTuber 本人、无意义、搜不到且只用过一两次的。用过 3 次以上的不要 drop，改成 keep。
- keep：搜到了但有歧义或拿不准，给 "note" 说明。

把结果写到当前目录的 proposals.json：一个数组，每项 {"tag", "action", 上面对应的字段, "evidence"}，evidence 用一句中文说明依据并附一个来源 URL。不要输出其他文件。stdout 只输出一行：各 action 的数量。"""


def research(run):
    run = pathlib.Path(run)
    subprocess.run([*GROK, "--cwd", str(run), "-p", PROMPT], check=True, timeout=3600,
                   stdout=open(run / "grok.log", "w"), stderr=subprocess.STDOUT)
    proposals = json.loads((run / "proposals.json").read_text())
    print(json.dumps({"proposals": len(proposals)}, ensure_ascii=False))


def merge_sql(f, t, key_f, key_t):
    F, T = sql_text(f), sql_text(t)
    return [
        f"INSERT OR IGNORE INTO tags (name, use_count) VALUES ({T}, 0);",
        f"INSERT OR IGNORE INTO work_tags (work_id, tag) SELECT work_id, {T} FROM work_tags WHERE tag = {F};",
        f"DELETE FROM work_tags WHERE tag = {F};",
        f"DELETE FROM tags WHERE name = {F};",
        f"DELETE FROM tag_counts WHERE tag = {F} AND cnt = 0;",
        f"UPDATE tags SET use_count = (SELECT COUNT(*) FROM work_tags WHERE tag = {T}) WHERE name = {T};",
        f"UPDATE tag_keys SET tag = {T} WHERE tag = {F};",
        f"INSERT INTO tag_keys (key, tag) VALUES ({sql_text(key_f)}, {T}) ON CONFLICT(key) DO UPDATE SET tag = excluded.tag;",
        f"INSERT INTO tag_keys (key, tag) VALUES ({sql_text(key_t)}, {T}) ON CONFLICT(key) DO UPDATE SET tag = excluded.tag;",
        f"UPDATE tag_items SET name = {T} WHERE name = {F} AND NOT EXISTS (SELECT 1 FROM tag_items WHERE name = {T});",
        f"DELETE FROM tag_items WHERE name = {F};",
        f"UPDATE tag_items SET parent = {T} WHERE parent = {F};",
        "UPDATE tag_items SET members = (SELECT json_group_array(DISTINCT CASE WHEN m.value = "
        f"{F} THEN {T} ELSE m.value END) FROM json_each(tag_items.members) m) WHERE members IS NOT NULL "
        f"AND EXISTS (SELECT 1 FROM json_each(tag_items.members) m WHERE m.value = {F});",
    ]


def block_sql(n):
    N = sql_text(n)
    return [
        f"INSERT OR IGNORE INTO tag_blocks (name) VALUES ({N});",
        f"DELETE FROM work_tags WHERE tag = {N};",
        f"DELETE FROM tags WHERE name = {N};",
        f"DELETE FROM tag_counts WHERE tag = {N} AND cnt = 0;",
        f"DELETE FROM tag_items WHERE name = {N};",
        f"UPDATE tag_items SET parent = NULL WHERE parent = {N};",
        f"UPDATE tag_items SET members = NULLIF((SELECT json_group_array(m.value) FROM json_each(tag_items.members) m "
        f"WHERE m.value <> {N}), '[]') WHERE members IS NOT NULL AND EXISTS (SELECT 1 FROM json_each(tag_items.members) m WHERE m.value = {N});",
    ]


def item_sql(name, kind, parent, ja, en, members):
    cell = lambda v: "NULL" if v is None else sql_text(v)  # noqa: E731
    m = json.dumps(members, ensure_ascii=False) if members else None
    return [f"INSERT INTO tag_items (name, kind, parent, ja, en, members) VALUES ({cell(name)}, {cell(kind)}, {cell(parent)}, "
            f"{cell(ja)}, {cell(en)}, {cell(m)}) ON CONFLICT(name) DO UPDATE SET kind = excluded.kind, parent = excluded.parent, "
            "ja = excluded.ja, en = excluded.en, members = excluded.members;"]


def plan(decisions, counts, library, blocked, keys, taken, kept=()):
    """Check every decision against the database as it is now and write the SQL for the ones that hold.

    counts {tag: visible works}, library {name: {kind, parent, ...}}, blocked {name}, keys {spelling: (key, stored
    form)}, taken {key: tag already holding it}, kept {tags the owner refused to drop}. Returns (statements,
    applied, refused); a drop of a non-Japanese tag is applied as "review" (no SQL) for the owner to settle."""
    applied, refused, statements, blocks, items = [], [], [], [], []
    parents = {n: e.get("parent") for n, e in library.items()}
    members_of = {n: e.get("members", []) for n, e in library.items()}
    names = set(counts) | set(library)
    renamed = {}  # candidate -> new name taken by an item in this batch
    for d in decisions:
        if d.get("action") == "item" and d.get("name") and d["name"] != d.get("tag"):
            renamed[d["tag"]] = d["name"]
    known = names | set(renamed.values())

    def refuse(d, why):
        refused.append({**d, "refused": why})

    def ancestors(name):
        """Every tag `name` implies through parents and members, as decided so far."""
        seen_names, todo = set(), [name]
        while todo:
            n = todo.pop()
            for up in [parents.get(n), *members_of.get(n, [])]:
                if up and up not in seen_names:
                    seen_names.add(up)
                    todo.append(up)
        return seen_names

    for d in decisions:
        tag, action = d.get("tag"), d.get("action")
        if tag not in counts or tag in library:
            refuse(d, "标签已不在线上，或已在标签库里（自动整理只处理未分类标签）")
            continue
        if action == "keep":
            applied.append(d)
            continue
        if action == "drop":
            if tag in kept:
                refuse(d, "站长已否决删除")
                continue
            if counts[tag] > DROP_MAX:
                refuse(d, f"用在 {counts[tag]} 件作品上，超过可自动删除的 {DROP_MAX} 件")
                continue
            if japanese(tag):
                blocks += block_sql(tag)
                applied.append(d)
            else:
                applied.append({**d, "action": "review", "uses": counts[tag]})
            continue
        if action == "merge":
            target = d.get("target")
            if not target or target == tag or target not in known or target in blocked:
                refuse(d, "合并目标不存在、已屏蔽或与原标签相同")
                continue
            holder = taken.get(keys[target][0])
            if holder not in (None, target, tag):
                refuse(d, f"「{target}」的写法已属于标签「{holder}」")
                continue
            statements += merge_sql(tag, target, keys[tag][0], keys[target][0])
            # Later decisions in the batch see the merged spelling as the target's (an item may take it as its English name).
            taken[keys[tag][0]] = taken[keys[target][0]] = target
            applied.append(d)
            continue
        if action != "item":
            refuse(d, "未知的处理方式")
            continue
        name, kind = d.get("name") or tag, d.get("kind")
        parent = d.get("parent") or None
        members = [m for m in d.get("members") or [] if m]
        if kind not in KINDS:
            refuse(d, "类型无效")
            continue
        if name != tag and (name in names or name in blocked or keys[name][1] != name):
            refuse(d, f"新名字「{name}」已存在、已屏蔽或不是规范写法；应改用 merge")
            continue
        if parent and (parent == name or parent not in known):
            refuse(d, f"所属「{parent}」不存在")
            continue
        if members and (kind != "cp" or len(members) > 8 or any(m not in known or m == name for m in members)):
            refuse(d, "CP 成员不存在或不合法")
            continue
        if (parent and name in ancestors(parent)) or any(name in ancestors(m) for m in members):
            refuse(d, "所属或成员会形成循环")
            continue
        ja, en = d.get("ja") or None, None if kind in NO_ENGLISH else d.get("en") or None
        dropped = []
        for side, value in (("ja", ja), ("en", en)):
            holder = taken.get(keys[value][0]) if value else None
            if value and holder not in (None, name, tag):
                dropped.append(f"{side}「{value}」已是「{holder}」的写法")
                if side == "ja":
                    ja = None
                else:
                    en = None
        if name != tag:
            statements += merge_sql(tag, name, keys[tag][0], keys[name][0])
            taken[keys[tag][0]] = taken[keys[name][0]] = name
        items += item_sql(name, kind, parent, ja, en, members)
        for value in (ja, en):
            if value:
                items.append(f"INSERT OR IGNORE INTO tag_keys (key, tag) VALUES ({sql_text(keys[value][0])}, {sql_text(name)});")
        parents[name] = parent
        members_of[name] = members
        names.add(name)
        applied.append({**d, "name": name, "ja": ja, "en": en, **({"note": "；".join(dropped)} if dropped else {})})
    statements += blocks + items
    if statements:
        statements += [s if s.endswith(";") else s + ";" for s in tag_merge.LINKS_SQL]
    return statements, applied, refused


def rehearse(backup, sql_path):
    db = sqlite3.connect(":memory:")
    db.executescript(pathlib.Path(backup).read_text())
    db.execute("PRAGMA foreign_keys = ON")
    db.executescript(pathlib.Path(sql_path).read_text())
    truth = dict(db.execute("SELECT wt.tag, COUNT(*) FROM work_tags wt JOIN works w ON w.id = wt.work_id "
                            "WHERE w.deleted_at IS NULL GROUP BY wt.tag"))
    assert {t: c for t, c in db.execute("SELECT tag, cnt FROM tag_counts") if c} == truth, "tag_counts drifted"
    links = set(db.execute("SELECT tag, also FROM tag_links"))
    assert links == set(db.execute("SELECT tag, also FROM tag_closure")), "tag_links differ from tag_closure"
    return db.execute("SELECT COUNT(*) FROM tag_counts c LEFT JOIN tag_items i ON i.name = c.tag "
                      "WHERE c.cnt > 0 AND i.name IS NULL").fetchone()[0]


def live_state():
    counts = {r["tag"]: r["cnt"] for r in d1("SELECT tag, cnt FROM tag_counts WHERE cnt > 0")}
    library = {r["name"]: {k: r[k] for k in ("kind", "parent") if r[k]} | {"members": json.loads(r["members"] or "[]")}
               for r in d1("SELECT name, kind, parent, members FROM tag_items")}
    return counts, library


def execute(statements, run, dry_run, report):
    """Back up, rehearse on the backup, then run on D1 and check the counts; notes go into report."""
    sql_path = run / "apply.sql"
    sql_path.write_text("\n".join(statements) + "\n")
    if not statements:
        return
    backup = run / "backup.sql"
    subprocess.run(["wrangler", "d1", "export", "vitrine", "--remote", "--output", str(backup)],
                   cwd=ROOT, capture_output=True, check=True)
    left = rehearse(backup, sql_path)
    report.append(f"演练通过：执行后还剩 {left} 个未分类标签。")
    if dry_run:
        return
    subprocess.run(["wrangler", "d1", "execute", "vitrine", "--remote", "--yes", "--file", str(sql_path)],
                   cwd=ROOT, capture_output=True, check=True)
    drift = d1("SELECT COUNT(*) AS n FROM (SELECT wt.tag, COUNT(*) n FROM work_tags wt JOIN works w ON w.id = wt.work_id "
               "WHERE w.deleted_at IS NULL GROUP BY wt.tag) t LEFT JOIN tag_counts c ON c.tag = t.tag WHERE c.cnt IS NOT t.n")[0]["n"]
    report.append(f"已写入线上；计数核对 {'一致' if drift == 0 else f'有 {drift} 个标签不一致'}。")
    # Old runs keep their reports; only the latest few keep their 60 MB backups.
    for old in sorted((STATE / "runs").glob("*/backup.sql"))[:-5]:
        old.unlink()


def pending_lines(pending):
    return [f"- {t}（{v['uses']} 件作品）：{v.get('note') or '无说明'}" for t, v in pending.items()]


def apply(run, dry_run=False):
    run = pathlib.Path(run)
    decisions = json.loads((run / "decisions.json").read_text())
    counts, library = live_state()
    blocked = {r["name"] for r in d1("SELECT name FROM tag_blocks")}
    spellings = {s for d in decisions for s in (d.get("tag"), d.get("target"), d.get("name"), d.get("ja"), d.get("en")) if s}
    keys = tag_merge.keys_for(sorted(spellings | set(counts) | set(library)))
    wanted = sorted({keys[s][0] for s in spellings})
    taken = {r["key"]: r["tag"] for start in range(0, len(wanted), 80)
             for r in d1(f"SELECT key, tag FROM tag_keys WHERE key IN {in_list(wanted[start:start + 80])}")}
    statements, applied, refused = plan(decisions, counts, library, blocked, keys, taken, set(stored("kept.json", [])))
    report = [f"# 标签整理 {run.name}", "", f"决定 {len(decisions)} 条：执行 {len(applied)} 条，拒绝 {len(refused)} 条。", ""]
    for title, action in (("合并", "merge"), ("归类", "item"), ("删除并屏蔽（日语标签，直接删除）", "drop"),
                          ("待站长审核的删除", "review"), ("暂不处理", "keep")):
        rows = [d for d in applied if d["action"] == action]
        if rows:
            report += [f"## {title}（{len(rows)}）", ""] + [
                f"- {d['tag']}" + (f" → {d['target']}" if action == "merge" else "")
                + (f" → {d['name']}（{d['kind']}" + (f"，属于 {d['parent']}" if d.get("parent") else "") + "）" if action == "item" else "")
                + (f"（{d['uses']} 件作品）" if action == "review" else "")
                + (f"：{d['note']}" if d.get("note") else "") for d in rows] + [""]
    if refused:
        report += [f"## 拒绝（{len(refused)}）", ""] + [f"- {d.get('tag')}（{d.get('action')}）：{d['refused']}" for d in refused] + [""]
    execute(statements, run, dry_run, report)
    if not dry_run:
        quiet = seen()
        today = datetime.date.today().isoformat()
        quiet.update({d["tag"]: today for d in applied if d["action"] == "keep"})
        quiet.update({d["tag"]: today for d in refused if d.get("tag") in counts})
        store("seen.json", quiet)
        pending = stored("pending_drops.json", {})
        pending.update({d["tag"]: {"uses": d["uses"], "note": d.get("note", ""), "run": run.name}
                        for d in applied if d["action"] == "review"})
        store("pending_drops.json", pending)
        if pending:
            report += ["", f"待审核的删除共 {len(pending)} 个（含以前的）。批准：`python3 tools/tag_curate.py approve-drops 标签…` 或 `--all`；"
                       "否决：`reject-drops 标签…`。"]
    (run / "report.md").write_text("\n".join(report) + "\n")
    print("\n".join(report))


def drops():
    pending = stored("pending_drops.json", {})
    print("\n".join(pending_lines(pending)) if pending else "没有待审核的删除。")


def settle_drops(tags, approve, dry_run=False):
    """approve: remove and block the chosen pending tags; otherwise keep them and never propose dropping them again."""
    pending = stored("pending_drops.json", {})
    chosen = list(pending) if tags == ["--all"] else tags
    unknown = [t for t in chosen if t not in pending]
    if unknown:
        sys.exit(f"不在待审核清单里：{'、'.join(unknown)}")
    if not approve:
        store("kept.json", sorted(set(stored("kept.json", [])) | set(chosen)))
        for t in chosen:
            del pending[t]
        store("pending_drops.json", pending)
        print(f"已保留 {len(chosen)} 个标签，以后不再提议删除：{'、'.join(chosen)}")
        return
    counts, library = live_state()
    gone = [t for t in chosen if t not in counts or t in library]
    statements = [sql for t in chosen if t not in gone for sql in block_sql(t)]
    if statements:
        statements += list(tag_merge.LINKS_SQL)
    run = STATE / "runs" / (datetime.datetime.now().strftime("%Y%m%dT%H%M%S") + "-drops")
    run.mkdir(parents=True)
    report = [f"# 审核删除 {run.name}", "", f"批准删除 {len(chosen) - len(gone)} 个：" + "、".join(t for t in chosen if t not in gone)]
    if gone:
        report.append(f"已不在线上或已归类、跳过：{'、'.join(gone)}")
    execute(statements, run, dry_run, report)
    if not dry_run:
        for t in chosen:
            del pending[t]
        store("pending_drops.json", pending)
    (run / "report.md").write_text("\n".join(report) + "\n")
    print("\n".join(report))


if __name__ == "__main__":
    command, *rest = sys.argv[1:]
    if command == "collect":
        collect()
    elif command == "research":
        research(rest[0])
    elif command == "apply":
        apply(rest[0], dry_run="--dry-run" in rest)
    elif command == "drops":
        drops()
    elif command in ("approve-drops", "reject-drops") and rest:
        settle_drops([t for t in rest if t != "--dry-run"], command == "approve-drops", "--dry-run" in rest)
    else:
        sys.exit(__doc__)

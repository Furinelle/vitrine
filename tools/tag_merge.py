"""Plan and generate the SQL that folds duplicate tags into one name.

Tags whose `tag_key` (case, full-width, punctuation, invisible characters and
Traditional/Simplified folded; computed by the Worker's own code through the
vitrine-tag-keys binary) is equal are merged into the spelling carried by the
most visible works, except spellings listed in tools/tag_aliases.json, which go
to their curated name: each game's Chinese/Japanese spellings -> its Chinese
title, English spellings -> its English name, and the two are linked in
`tag_links` so every work carries both. tools/tag_fixes.json adds plain
misspelling fixes and a `keep` list (CP names and the like) that must never be
merged or renamed; the run stops if any would be. The output SQL also records every key
in `tag_keys`, which ingest and admin edits use to keep new variants under the
same name.

    python3 tools/tag_merge.py rows.json plan.md merge.sql

rows.json is the `wrangler d1 execute --json` output of ROWS_SQL. Review
plan.md, then apply merge.sql with `wrangler d1 execute DB --remote --file`.
Re-running on a merged database yields only `tag_keys` upserts.
"""
import collections
import json
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
ROWS_SQL = (
    "SELECT t.name, COALESCE((SELECT cnt FROM tag_counts WHERE tag=t.name),0) AS visible, "
    "(SELECT COUNT(*) FROM work_tags WHERE tag=t.name) AS total FROM tags t"
)
TRAD = {chr(int(a, 16)) for a, _ in re.findall(r"\('\\u\{([0-9A-F]+)\}', '\\u\{([0-9A-F]+)\}'\)",
                                               (ROOT / "src/tag_fold.rs").read_text())}


def sql_text(value):
    return "'" + value.replace("'", "''") + "'"


def keys_for(names):
    subprocess.run(["cargo", "build", "-q", "--bin", "vitrine-tag-keys"], cwd=ROOT, check=True)
    out = subprocess.run([str(ROOT / "target/debug/vitrine-tag-keys")], input="\n".join(names) + "\n",
                         capture_output=True, text=True, check=True).stdout
    rows = [line.split("\t") for line in out.splitlines()]
    assert len(rows) == len(names) and all(len(r) == 3 and r[2] == n for r, n in zip(rows, names))
    return {name: (key, stored) for key, stored, name in rows}


def alias_targets(aliases, keys):
    """{key: name} for every spelling listed in tag_aliases.json (name keys itself)."""
    out = {}
    for name, spellings in aliases.items():
        assert keys[name][1] == name, f"alias target is not a stored tag form: {name!r}"
        for spelling in [name, *spellings]:
            key = keys[spelling][0]
            assert out.setdefault(key, name) == name, f"{spelling!r} maps to both {out[key]!r} and {name!r}"
    return out


def plan(rows, keys, aliases=None):
    """Returns ({variant: target}, {key: target}, groups) for every tag name in rows.

    Keys listed in tag_aliases.json go to their curated name; any other group keeps
    the spelling with the most visible works.
    """
    fixed = alias_targets(aliases or {}, keys)
    groups = collections.defaultdict(list)
    for row in rows:
        groups[keys[row["name"]][0]].append(row)
    merges, targets, report = {}, dict(fixed), []
    for key, members in groups.items():
        # Most visible works, then most works overall, then Simplified, then already clean, then stable.
        best = min(members, key=lambda r: (-r["visible"], -r["total"], any(c in TRAD for c in r["name"]),
                                           keys[r["name"]][1] != r["name"], r["name"]))
        target = fixed.get(key, keys[best["name"]][1])
        targets[key] = target
        moved = [r for r in members if r["name"] != target]
        for r in moved:
            merges[r["name"]] = target
        if moved:
            report.append((target, members))
    # One report row per target: alias groups gather several keys.
    merged = collections.defaultdict(list)
    for target, members in report:
        merged[target].extend(members)
    return merges, targets, [(t, sorted(m, key=lambda r: (-r["visible"], r["name"]))) for t, m in merged.items()]


def merge_sql(merges, targets, links=()):
    lines = []
    if merges:
        lines += ["CREATE TABLE tag_merge (name TEXT PRIMARY KEY, tag TEXT NOT NULL);",
                  "INSERT INTO tag_merge (name, tag) VALUES"]
        lines += [f"  ({sql_text(a)}, {sql_text(b)})" + ("," if i < len(merges) - 1 else ";")
                  for i, (a, b) in enumerate(sorted(merges.items()))]
        lines += [
            "INSERT OR IGNORE INTO tags (name, use_count) SELECT DISTINCT tag, 0 FROM tag_merge;",
            "INSERT OR IGNORE INTO work_tags (work_id, tag) SELECT wt.work_id, m.tag FROM work_tags wt JOIN tag_merge m ON m.name = wt.tag;",
            "DELETE FROM work_tags WHERE tag IN (SELECT name FROM tag_merge);",
            "DELETE FROM tags WHERE name IN (SELECT name FROM tag_merge);",
            "DELETE FROM tag_counts WHERE tag IN (SELECT name FROM tag_merge) AND cnt = 0;",
            "UPDATE tags SET use_count = (SELECT COUNT(*) FROM work_tags WHERE tag = tags.name) WHERE name IN (SELECT tag FROM tag_merge);",
            "DROP TABLE tag_merge;",
        ]
    if links:
        pairs = sorted({p for zh, en in links for p in ((zh, en), (en, zh))})
        lines += ["DELETE FROM tag_links;", "INSERT INTO tag_links (tag, also) VALUES"]
        lines += [f"  ({sql_text(a)}, {sql_text(b)})" + ("," if i < len(pairs) - 1 else ";")
                  for i, (a, b) in enumerate(pairs)]
        lines += [
            "INSERT OR IGNORE INTO tags (name, use_count) SELECT DISTINCT tag, 0 FROM tag_links;",
            # Existing works get their companion tag; new writes go through the 0017 trigger.
            "INSERT OR IGNORE INTO work_tags (work_id, tag) SELECT wt.work_id, l.also FROM work_tags wt JOIN tag_links l ON l.tag = wt.tag;",
            "UPDATE tags SET use_count = (SELECT COUNT(*) FROM work_tags WHERE tag = tags.name) WHERE name IN (SELECT tag FROM tag_links);",
        ]
    items = sorted(targets.items())
    for start in range(0, len(items), 200):
        chunk = items[start:start + 200]
        lines.append("INSERT INTO tag_keys (key, tag) VALUES")
        lines += [f"  ({sql_text(k)}, {sql_text(t)})" + ("," if i < len(chunk) - 1 else "")
                  for i, (k, t) in enumerate(chunk)]
        lines.append("ON CONFLICT(key) DO UPDATE SET tag = excluded.tag;")
    return "\n".join(lines) + "\n"


def show(name):
    return "".join(f"<U+{ord(c):04X}>" if not c.isprintable() or c in "\u3164\u200c\u200b" else c for c in name)


def load_aliases(path=ROOT / "tools/tag_aliases.json"):
    """Returns ({stored name: [spellings]}, [(Chinese title, English name)]).

    Chinese and Japanese spellings go to the Chinese title, English ones and
    abbreviations to the English name; the two are linked as companion tags.
    """
    names, links = {}, []
    for zh, game in json.loads(pathlib.Path(path).read_text()).items():
        names[zh] = game["zh_aliases"]
        names[game["en"]] = game["en_aliases"]
        links.append((zh, game["en"]))
    return names, links


def load_fixes(path=ROOT / "tools/tag_fixes.json"):
    """Returns ({correct name: [misspellings]}, [names that must never be merged or renamed])."""
    data = json.loads(pathlib.Path(path).read_text())
    return data["fixes"], data["keep"]


def main(rows_path, plan_path, sql_path):
    rows = json.loads(pathlib.Path(rows_path).read_text())[0]["results"]
    aliases, links = load_aliases()
    fixes, keep = load_fixes()
    assert not set(fixes) & set(aliases), "a fix target is already a game name"
    aliases = {**aliases, **fixes}
    spellings = [s for name, more in aliases.items() for s in (name, *more)]
    keys = keys_for(sorted({r["name"] for r in rows} | set(spellings) | set(keep)))
    merges, targets, report = plan(rows, keys, aliases)
    # CP and other protected tags stay exactly as they are.
    for name in keep:
        assert name not in merges and targets.get(keys[name][0], name) == name, f"protected tag would change: {name!r}"
    by_name = {r["name"]: r for r in rows}
    report.sort(key=lambda item: (item[0] not in dict(links) and item[0] not in {en for _, en in links},
                                  item[0] not in fixes, -sum(r["visible"] for r in item[1])))
    out = [f"# 标签合并清单\n\n{len(rows)} 个标签，{len(report)} 组合并，{len(merges)} 个写法并入保留名。"
           "括号里是可见作品数 / 全部作品数，第一列是保留的写法。前面是 tools/tag_aliases.json 里的游戏名，"
           "其余按可见作品最多的写法保留。\n",
           "| 保留 | 并入 |", "| --- | --- |"]
    for target, members in report:
        moved = [r for r in members if r["name"] != target]
        kept = by_name.get(target)
        head = f"{show(target)} ({kept['visible']}/{kept['total']})" if kept else f"{show(target)}（新名）"
        out.append(f"| {head} | " + "、".join(f"{show(r['name'])} ({r['visible']}/{r['total']})" for r in moved) + " |")
    out += ["", "## 中英配对", "", "带其中一个标签的作品会同时带上另一个。括号里是现在的可见作品数，没有括号的是新建标签。", "",
            "| 中文 | 英文 |", "| --- | --- |"]
    label = lambda n: f"{n} ({by_name[n]['visible']})" if n in by_name else n
    out += [f"| {label(zh)} | {label(en)} |" for zh, en in links]
    pathlib.Path(plan_path).write_text("\n".join(out) + "\n")
    pathlib.Path(sql_path).write_text(merge_sql(merges, targets, links))
    print(f"{len(rows)} tags, {len(report)} groups, {len(merges)} names merged; plan -> {plan_path}, sql -> {sql_path}")


if __name__ == "__main__":
    main(*sys.argv[1:4])

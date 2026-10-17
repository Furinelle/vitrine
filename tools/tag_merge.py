"""Plan and generate the SQL that keeps the tag library in line with its JSON sources.

Tags whose `tag_key` (case, full-width, punctuation, invisible characters and
Traditional/Simplified folded; computed by the Worker's own code through the
vitrine-tag-keys binary) is equal are merged into the spelling carried by the
most visible works. tools/tag_aliases.json is the tag library: one entry per
tag, stored under its Chinese name, with its kind, the work it belongs to, CP
members and its Japanese / English display names. Every display name and listed
spelling folds into the stored name; `tag_items` keeps the display names, and
`tag_links` makes a work carry what its tags imply (character -> work, CP ->
characters, AI tools -> AI生成). tools/tag_fixes.json adds misspelling fixes,
`implies`, a `drop` list (removed and blocked) and a `keep` list (CP names and
the like) that must never be merged or renamed; the run stops if any would be.
The output SQL also records every key in `tag_keys`, which ingest and admin
edits use to keep new variants under the same name.

    python3 tools/tag_merge.py rows.json plan.md merge.sql

rows.json is the `wrangler d1 execute --json` output of ROWS_SQL. Review
plan.md, then apply merge.sql with `wrangler d1 execute DB --remote --file`.
Re-running on an up-to-date database changes nothing.
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


def merge_sql(merges, targets, implies=(), drops=(), items=()):
    """implies: (name, implied name) pairs, already closed over chains; drops: names removed
    from every work and blocked from coming back (migration 0018); items: tag library rows
    (name, kind, parent, ja, en) for migration 0019."""
    lines = []
    # Blocks first: a name unblocked this run can then receive merged tags.
    if drops:
        lines += ["DELETE FROM tag_blocks;", "INSERT INTO tag_blocks (name) VALUES"]
        lines += [f"  ({sql_text(n)})" + ("," if i < len(drops) - 1 else ";") for i, n in enumerate(sorted(drops))]
        lines += [
            "DELETE FROM work_tags WHERE tag IN (SELECT name FROM tag_blocks);",
            "DELETE FROM tags WHERE name IN (SELECT name FROM tag_blocks);",
            "DELETE FROM tag_counts WHERE tag IN (SELECT name FROM tag_blocks) AND cnt = 0;",
        ]
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
    if items:
        lines += ["DELETE FROM tag_items;", "INSERT INTO tag_items (name, kind, parent, ja, en) VALUES"]
        cell = lambda v: "NULL" if v is None else sql_text(v)
        lines += [f"  ({', '.join(cell(v) for v in row)})" + ("," if i < len(items) - 1 else ";")
                  for i, row in enumerate(sorted(items))]
    lines.append("DELETE FROM tag_links;")
    if implies:
        pairs = sorted(set(implies))
        lines += ["INSERT INTO tag_links (tag, also) VALUES"]
        lines += [f"  ({sql_text(a)}, {sql_text(b)})" + ("," if i < len(pairs) - 1 else ";")
                  for i, (a, b) in enumerate(pairs)]
        lines += [
            "INSERT OR IGNORE INTO tags (name, use_count) SELECT tag, 0 FROM tag_links UNION SELECT also, 0 FROM tag_links;",
            # Existing works get the implied tags; new writes go through the 0017 trigger.
            "INSERT OR IGNORE INTO work_tags (work_id, tag) SELECT wt.work_id, l.also FROM work_tags wt JOIN tag_links l ON l.tag = wt.tag;",
            "UPDATE tags SET use_count = (SELECT COUNT(*) FROM work_tags WHERE tag = tags.name) WHERE name IN (SELECT tag FROM tag_links UNION SELECT also FROM tag_links);",
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


def load_library(path=ROOT / "tools/tag_aliases.json"):
    """Returns ({stored name: [spellings]}, [(name, kind, parent, ja, en)], [(name, implied name)]).

    Each entry is one tag, stored under its Chinese name. `ja` (Japanese works and
    characters) and `en` (only a common, instantly recognisable English name) are
    display names: they and every listed spelling fold into the stored name.
    `parent` (the work a character or setting belongs to) and CP `members` are
    implied: a work tagged with the entry also gets them.
    """
    names, items, edges = {}, [], []
    for zh, entry in json.loads(pathlib.Path(path).read_text()).items():
        names[zh] = [*entry.get("zh_aliases", []),
                     *[n for side in ("ja", "en") if entry.get(side) for n in (entry[side], *entry.get(side + "_aliases", []))]]
        items.append((zh, entry["kind"], entry.get("parent"), entry.get("ja"), entry.get("en")))
        edges += [(zh, entry["parent"])] if entry.get("parent") else []
        edges += [(zh, m) for m in entry.get("members", [])]
    return names, items, edges


def closure(edges):
    """(tag, implied) for every tag reachable through implication chains (CP -> character -> work)."""
    nxt = collections.defaultdict(set)
    for a, b in edges:
        nxt[a].add(b)
    out = set()
    for start in list(nxt):
        stack, seen = list(nxt[start]), set()
        while stack:
            b = stack.pop()
            if b in seen or b == start:
                continue
            seen.add(b)
            out.add((start, b))
            stack += nxt.get(b, ())
    return sorted(out)


def load_fixes(path=ROOT / "tools/tag_fixes.json"):
    """Returns (fixes {correct name: [misspellings]}, keep [never merged or renamed],
    drop [removed and blocked], implies {name: [tags whose works also get it]})."""
    data = json.loads(pathlib.Path(path).read_text())
    return data["fixes"], data["keep"], data.get("drop", []), data.get("implies", {})


def main(rows_path, plan_path, sql_path):
    rows = json.loads(pathlib.Path(rows_path).read_text())[0]["results"]
    aliases, items, edges = load_library()
    fixes, keep, drops, implied_by = load_fixes()
    # A fix target may be a library tag or one of its display names (Vodyanitsa); the misspellings
    # then fold into the library tag's stored name as well.
    owner = {s: name for name, more in aliases.items() for s in more}
    for name, spellings in fixes.items():
        dest = owner.get(name, name)
        aliases[dest] = aliases.get(dest, []) + ([name] if dest != name else []) + spellings
    spellings = [s for name, more in aliases.items() for s in (name, *more)]
    keys = keys_for(sorted({r["name"] for r in rows} | set(spellings) | set(keep)))
    merges, targets, report = plan(rows, keys, aliases)
    library = {i[0] for i in items}
    assert not set(drops) & (set(keep) | library), "a dropped tag is protected or in the library"
    # A source that is merged away or dropped has no works left to imply anything for.
    edges += [(src, name) for name, sources in implied_by.items() for src in sources if src not in merges and src not in drops]
    implies = closure(edges)
    assert all(b not in merges and b not in drops for _, b in implies), "an implied tag is merged away or dropped"
    # CP and other protected tags stay exactly as they are.
    for name in keep:
        assert name not in merges and targets.get(keys[name][0], name) == name, f"protected tag would change: {name!r}"
    by_name = {r["name"]: r for r in rows}
    report.sort(key=lambda item: (item[0] not in library, item[0] not in fixes, -sum(r["visible"] for r in item[1])))
    out = [f"# 标签合并清单\n\n{len(rows)} 个标签，{len(report)} 组合并，{len(merges)} 个写法并入保留名。"
           "括号里是可见作品数 / 全部作品数，第一列是保留的写法。\n",
           "| 保留 | 并入 |", "| --- | --- |"]
    for target, members in report:
        moved = [r for r in members if r["name"] != target]
        kept = by_name.get(target)
        head = f"{show(target)} ({kept['visible']}/{kept['total']})" if kept else f"{show(target)}（新名）"
        out.append(f"| {head} | " + "、".join(f"{show(r['name'])} ({r['visible']}/{r['total']})" for r in moved) + " |")
    label = lambda n: f"{n} ({by_name[n]['visible']})" if n in by_name else f"{n}（新建）"
    kinds = collections.Counter(i[1] for i in items)
    out += ["", "## 标签库", "", "、".join(f"{k} {v} 个" for k, v in kinds.most_common()), ""]
    out += [f"- {label(n)} · {k}" + (f" · 属于 {p}" if p else "") + "".join(f" · {s} {v}" for s, v in (("日文", ja), ("英文", en)) if v)
            for n, k, p, ja, en in items]
    out += ["", f"## 删除并屏蔽（{len(drops)} 个）", "", "、".join(label(n) for n in sorted(drops, key=lambda n: -by_name.get(n, {}).get("visible", 0)))]
    out += ["", f"## 自动补标（{len(implies)} 条）", ""] + [f"- 带 {a} 的作品同时带上 {b}" for a, b in implies]
    pathlib.Path(plan_path).write_text("\n".join(out) + "\n")
    pathlib.Path(sql_path).write_text(merge_sql(merges, targets, implies, drops, items))
    print(f"{len(rows)} tags, {len(report)} groups, {len(merges)} names merged, {len(items)} library tags, "
          f"{len(implies)} implications; plan -> {plan_path}, sql -> {sql_path}")


if __name__ == "__main__":
    main(*sys.argv[1:4])

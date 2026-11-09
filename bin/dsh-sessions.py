#!/usr/bin/env python3
"""Быстрый локальный поиск сессий DSH: id, title, cwd — без HTTP API.

Читает кэш DSH `~/.dsh/storages/session_projcache/sessions/*.json`
(identity.cwd + rows.title.val). Нужен, чтобы не нагружать
`/api/session/list` (он отвечает до 120 с при высокой параллельности).

Использование: bin/dsh-sessions.py [path-substr]
"""
import glob, json, os, sys

CACHE = os.path.expanduser("~/.dsh/storages/session_projcache/sessions")

def main():
    pat = next((a for a in sys.argv[1:] if not a.startswith("-")), "")
    rows = []
    for f in glob.glob(os.path.join(CACHE, "*.json")):
        try:
            d = json.load(open(f))
        except Exception:
            continue
        rec = d.get("record", {})
        cwd = str((rec.get("identity") or {}).get("cwd", ""))
        title = ((rec.get("rows") or {}).get("title") or {}).get("val") or ""
        if pat and pat not in cwd:
            continue
        sid = os.path.basename(f)[:-5]
        rows.append((cwd, sid, title))
    for cwd, sid, title in sorted(rows):
        print(f"{sid}\t{title}\t{cwd}")

if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""DSH API: отправка сообщения в любую сессию и поиск сессий по cwd.

Токен и порт берутся из /tmp/dsh.log (строка 'dsh web: URL?token=...'), т.е. DSH
должен быть запущен из терминала с `... 2>&1 | tee /tmp/dsh.log`.

Использование:
  bin/dsh-api.py list [path-substr]        # список сессий (id, cwd, preset, running)
  bin/dsh-api.py prompt <sessionId> <text> # отправить сообщение в сессию
"""
import json, os, re, subprocess, sys, time, uuid, urllib.request, urllib.error, http.cookiejar

LOG = "/tmp/dsh.log"
COOKIE = "/tmp/dsh_cookies.txt"

def base_url():
    try:
        text = open(LOG, encoding="utf-8", errors="replace").read()
    except FileNotFoundError:
        sys.exit("нет /tmp/dsh.log — запусти DSH из терминала: \"...\" 2>&1 | tee /tmp/dsh.log")
    m = None
    for m in re.finditer(r"dsh web:\s+(http://127\.0\.0\.1:\d+)/\?token=([A-Za-z0-9_-]+)", text):
        pass
    if m is None:
        sys.exit("в /tmp/dsh.log нет строки 'dsh web: http://127.0.0.1:PORT/?token=...'")
    return m.group(1), m.group(2)

def mint_cookie(url, token):
    jar = http.cookiejar.MozillaCookieJar(COOKIE)
    opener = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(jar))
    class NoRedirect(urllib.request.HTTPRedirectHandler):
        def redirect_request(self, *a, **k): return None
    opener = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(jar), NoRedirect)
    req = urllib.request.Request(f"{url}/?token={token}")
    try:
        opener.open(req)
    except urllib.error.HTTPError:
        pass
    jar.save(ignore_discard=True, ignore_expires=True)

def rpc(url, endpoint, args):
    body = json.dumps({"type": "client-request", "rpcId": str(uuid.uuid4()), "method": endpoint,
                       "payload": {"args": args}}).encode()
    req = urllib.request.Request(f"{url}/api/{endpoint}", data=body,
                                 headers={"content-type": "application/json", "cookie": cookie_header()})
    last = None
    for attempt in range(1, 6):
        try:
            with urllib.request.urlopen(req, timeout=180) as r:
                data = json.loads(r.read())
            break
        except Exception as error:
            last = error
            if attempt == 5:
                sys.exit(f"API transport failed after 5 attempts: {error}")
            time.sleep(min(2 ** attempt, 15))
    else:
        sys.exit(f"API transport failed: {last}")
    res = data.get("result", {})
    if not res.get("ok"):
        sys.exit(f"API error: {json.dumps(res.get('error'), ensure_ascii=False)}")
    return res.get("value")

def cookie_header():
    text = open(COOKIE).read()
    parts = [ln.split("\t")[-2] + "=" + ln.split("\t")[-1] for ln in text.splitlines() if ln and not ln.startswith("#")]
    return "; ".join(parts)

def main():
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    url, token = base_url()
    mint_cookie(url, token)
    cmd = sys.argv[1]
    if cmd == "list":
        val = rpc(url, "session/list", {"_request": {}})
        items = val.get("items", [])
        pat = sys.argv[2] if len(sys.argv) > 2 else ""
        for it in items:
            cwd = it.get("cwd", "")
            if pat and pat not in cwd:
                continue
            title = ((it.get("projections") or {}).get("values") or {}).get("title", "")
            print(f"{it['sessionId']}\trunning={it.get('running')}\t{it.get('agentPreset') or ''}\t{cwd}\t{title}")
    elif cmd == "prompt":
        sid, text = sys.argv[2], sys.argv[3]
        rid = "aif-" + uuid.uuid4().hex[:8]
        rpc(url, "session/prompt", {"request": {"requestId": rid, "sessionId": sid, "mode": "queue",
                                               "content": [{"type": "text", "text": text}]}})
        print(f"sent -> {sid}")
    else:
        sys.exit(__doc__)

if __name__ == "__main__":
    main()

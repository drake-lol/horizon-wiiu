#!/usr/bin/env python3
# Mirror a local folder onto the Wii U's SD card over FTP (ftpiiu) with several connections, skipping files already
# there with the same size. usage: sdsync.py <local dir> <remote dir> [skip,exts] [workers]
import ftplib, os, sys, time, threading, queue
HOST = os.environ.get("WIIU", "192.168.0.10")
src, dst = os.path.expanduser(sys.argv[1]), sys.argv[2]
skip_ext = tuple(e for e in (sys.argv[3].split(",") if len(sys.argv) > 3 else []) if e)
workers = int(sys.argv[4]) if len(sys.argv) > 4 else 4
def conn():
    f = ftplib.FTP(); f.connect(HOST, 21, timeout=60); f.login(); return f
main = conn()
def ensure(d):
    try: main.mkd(d)
    except ftplib.error_perm: pass
def existing(d):
    out = {}
    try:
        for name, facts in main.mlsd(d, ["size", "type"]):
            if facts.get("type") == "file": out[name] = int(facts.get("size", -1))
    except Exception:
        pass
    return out
parts = dst.strip("/").split("/")
for i in range(1, len(parts) + 1): ensure("/" + "/".join(parts[:i]))
q = queue.Queue(maxsize=2000)
stats = {"files": 0, "bytes": 0, "fail": 0}; lock = threading.Lock(); t0 = time.time()
def worker():
    f = conn()
    while True:
        item = q.get()
        if item is None: return
        p, remote, size = item
        for attempt in range(6):
            try:
                with open(p, "rb") as fh: f.storbinary("STOR " + remote, fh, 65536)
                break
            except Exception as e:
                time.sleep(3)
                try: f = conn()
                except Exception: pass
        else:
            with lock: stats["fail"] += 1
            print("FAILED", remote, flush=True)
        with lock:
            stats["files"] += 1; stats["bytes"] += size
            if stats["files"] % 500 == 0:
                print(f"{stats['files']} files, {stats['bytes']/1e6:.0f} MB sent, {time.time()-t0:.0f} s", flush=True)
ts = [threading.Thread(target=worker, daemon=True) for _ in range(workers)]
for t in ts: t.start()
for root, dirs, files in os.walk(src, followlinks=True):
    dirs.sort()
    rel = os.path.relpath(root, src)
    rd = dst if rel == "." else dst + "/" + rel.replace(os.sep, "/")
    ensure(rd)
    have = existing(rd)
    for name in sorted(files):
        if skip_ext and name.endswith(skip_ext): continue
        p = os.path.join(root, name); size = os.path.getsize(p)
        if have.get(name) == size: continue
        q.put((p, rd + "/" + name, size))
for _ in ts: q.put(None)
for t in ts: t.join()
print(f"DONE {stats['files']} files, {stats['bytes']/1e6:.0f} MB sent, {stats['fail']} failed, {time.time()-t0:.0f} s", flush=True)

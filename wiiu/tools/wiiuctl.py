#!/usr/bin/env python3
"""Drive a real Wii U (Aroma) from a computer: screenshots, GamePad input, power, running homebrew.

Talks to the Horizon test bridge plugin (wiiu/testbridge, TCP 4410) and to Aroma's own plugins: wiiload
(TCP 4299) to send and start an .rpx/.wuhb, ftpiiu (TCP 21) to copy files to the SD card, and the logging
module's UDP broadcast (4405) for console output. Standard library only.

    export WIIU=192.168.1.50            # or --host
    wiiuctl.py ping
    wiiuctl.py shot tv.png              # --drc for the GamePad, --scale 2 for half size
    wiiuctl.py press A                  # buttons: A B X Y L R ZL ZR PLUS MINUS HOME UP DOWN LEFT RIGHT STICK_L STICK_R
    wiiuctl.py press ZR+B 500           # several at once, for 500 ms
    wiiuctl.py hold ZR ; wiiuctl.py release ZR
    wiiuctl.py stick L 0 1 1000         # left stick fully up for a second
    wiiuctl.py touch 427 240            # tap the middle of the GamePad screen
    wiiuctl.py run horizon-real.rpx     # send and start (Aroma's wiiload plugin)
    wiiuctl.py logs                     # print console output (Aroma's logging module)
    wiiuctl.py menu | reboot | shutdown | launch 0005000010101e00
    wiiuctl.py install HorizonTestBridge.wps   # copy the plugin to the SD card (ftpiiu), then reboot
    wiiuctl.py script test.txt          # one command per line, plus "wait <ms>" and "expect-ping"

As a library: `from wiiuctl import WiiU; w = WiiU("192.168.1.50"); w.press("A"); w.shot("x.png")`.
"""

import argparse
import ftplib
import io
import os
import socket
import struct
import sys
import time
import zlib

BRIDGE_PORT = 4410
WIILOAD_PORT = 4299
LOG_PORT = 4405
PLUGIN_DIR = "/fs/vol/external01/wiiu/environments/aroma/plugins"


def write_png(path, width, height, rgb):
    """An 8-bit RGB PNG from raw rows."""
    raw = b"".join(b"\x00" + rgb[y * width * 3 : (y + 1) * width * 3] for y in range(height))

    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)

    png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(raw, 6)) + chunk(b"IEND", b"")
    with open(path, "wb") as f:
        f.write(png)


class BridgeError(Exception):
    pass


class WiiU:
    """A connection to the test bridge. Reconnects when the console switches application (the plugin's server
    restarts with each one)."""

    def __init__(self, host, timeout=10.0):
        self.host = host
        self.timeout = timeout
        self.sock = None
        self.buf = b""

    # -- connection --

    def connect(self, wait=0.0):
        deadline = time.time() + wait
        while True:
            try:
                self.sock = socket.create_connection((self.host, BRIDGE_PORT), timeout=self.timeout)
                self.buf = b""
                hello = self._line()
                if not hello.startswith("OK horizon-testbridge"):
                    raise BridgeError(f"not the test bridge: {hello!r}")
                return hello
            except (OSError, BridgeError):
                self.close()
                if time.time() >= deadline:
                    raise
                time.sleep(1.0)

    def close(self):
        if self.sock:
            try:
                self.sock.close()
            except OSError:
                pass
        self.sock = None

    def _line(self):
        while b"\n" not in self.buf:
            data = self.sock.recv(65536)
            if not data:
                raise BridgeError("connection closed")
            self.buf += data
        line, self.buf = self.buf.split(b"\n", 1)
        return line.decode("utf-8", "replace").rstrip("\r")

    def _exact(self, n):
        while len(self.buf) < n:
            data = self.sock.recv(1 << 20)
            if not data:
                raise BridgeError("connection closed")
            self.buf += data
        out, self.buf = self.buf[:n], self.buf[n:]
        return out

    def command(self, line, expect_reply=True):
        if self.sock is None:
            self.connect()
        try:
            self.sock.sendall(line.encode() + b"\n")
            if not expect_reply:
                return ""
            reply = self._line()
        except (OSError, BridgeError):
            # the application changed under us: once more on a fresh connection
            self.close()
            self.connect(wait=self.timeout)
            self.sock.sendall(line.encode() + b"\n")
            reply = self._line()
        if reply.startswith("ERR"):
            raise BridgeError(reply[4:])
        return reply[3:] if reply.startswith("OK ") else ""

    # -- commands --

    def ping(self):
        return self.command("PING")

    def info(self):
        fields = dict(f.split("=", 1) for f in self.command("INFO").split() if "=" in f)
        return fields

    def shot_rgb(self, drc=False, scale=1):
        """(width, height, rgb bytes, warning) of the next frame shown on the TV or the GamePad."""
        head = self.command(f"SHOT {'DRC' if drc else 'TV'} {scale}").split()
        if len(head) < 4 or head[0] != "IMG":
            raise BridgeError(f"unexpected reply {head}")
        w, h, n = int(head[1]), int(head[2]), int(head[3])
        return w, h, self._exact(n), " ".join(head[4:])

    def shot(self, path, drc=False, scale=1):
        w, h, rgb, warning = self.shot_rgb(drc, scale)
        write_png(path, w, h, rgb)
        return w, h, warning

    def press(self, buttons, ms=150):
        return self.command(f"PRESS {buttons} {ms}")

    def hold(self, buttons):
        return self.command(f"HOLD {buttons}")

    def release(self, buttons="ALL"):
        return self.command(f"RELEASE {buttons}")

    def stick(self, which, x, y, ms=0):
        return self.command(f"STICK {which} {x} {y} {ms}")

    def touch(self, x, y, ms=150):
        return self.command(f"TOUCH {int(x)} {int(y)} {ms}")

    def clear(self):
        return self.command("CLEAR")

    def menu(self):
        return self.command("MENU")

    def launch(self, title_id):
        return self.command(f"LAUNCH {title_id}")

    def reboot(self):
        return self.command("REBOOT")

    def shutdown(self):
        return self.command("SHUTDOWN")

    # -- Aroma's plugins --

    def run(self, path, args=()):
        """Send an .rpx or .wuhb to Aroma's wiiload plugin, which starts it."""
        data = open(path, "rb").read()
        packed = zlib.compress(data, 6)
        argv = b"".join(a.encode() + b"\0" for a in [os.path.basename(path), *args])
        with socket.create_connection((self.host, WIILOAD_PORT), timeout=30) as s:
            s.sendall(b"HAXX" + bytes([0, 5]) + struct.pack(">H", len(argv)))
            s.sendall(struct.pack(">II", len(packed), len(data)))
            s.sendall(packed)
            s.sendall(argv)
        self.close()

    def runsd(self, path):
        """Start an .rpx/.wuhb already on the SD card; `path` from the card's root (wiiu/apps/x/x.rpx)."""
        reply = self.command(f"RUNSD {path}")
        self.close()
        return reply

    def deploy(self, local, sd_dir="wiiu/apps/horizon"):
        """Copy an .rpx/.wuhb to the SD card over FTP and start it (no Wiiload needed)."""
        self.upload(local, "/fs/vol/external01/" + sd_dir)
        return self.runsd(f"{sd_dir}/{os.path.basename(local)}")

    def fetch(self, remote, local, user="anonymous", password=""):
        """Copy a file from the console over FTP; `remote` from the SD card's root."""
        with ftplib.FTP() as ftp:
            ftp.connect(self.host, 21, timeout=30)
            ftp.login(user, password)
            with open(local, "wb") as f:
                ftp.retrbinary(f"RETR /fs/vol/external01/{remote.lstrip('/')}", f.write)

    def upload(self, local, remote_dir, name=None, user="anonymous", password=""):
        """Copy a file to the console over FTP (Aroma's ftpiiu plugin), making the folder if needed."""
        with ftplib.FTP() as ftp:
            ftp.connect(self.host, 21, timeout=30)
            ftp.login(user, password)
            parts = remote_dir.strip("/").split("/")
            for i in range(1, len(parts) + 1):
                try:
                    ftp.mkd("/" + "/".join(parts[:i]))
                except ftplib.error_perm:
                    pass
            ftp.cwd(remote_dir)
            with open(local, "rb") as f:
                ftp.storbinary(f"STOR {name or os.path.basename(local)}", f)


def logs(seconds=None):
    """Print what Aroma's logging module broadcasts (UDP 4405) until interrupted or `seconds` pass."""
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    s.bind(("", LOG_PORT))
    end = time.time() + seconds if seconds else None
    while end is None or time.time() < end:
        s.settimeout(1.0 if end else None)
        try:
            data, _ = s.recvfrom(65536)
        except socket.timeout:
            continue
        sys.stdout.write(data.decode("utf-8", "replace"))
        sys.stdout.flush()


def run_script(w, path):
    """One command per line: any bridge command (PRESS A, SHOT TV ...), or `wait <ms>`, `shot <file> [drc]`,
    `run <file.rpx>`, `expect-ping [seconds]`. `#` starts a comment."""
    for n, raw in enumerate(open(path), 1):
        line = raw.split("#", 1)[0].strip()
        if not line:
            continue
        word, *rest = line.split()
        word = word.lower()
        print(f"{n}: {line}")
        if word == "wait":
            time.sleep(int(rest[0]) / 1000)
        elif word == "shot":
            print("   ", w.shot(rest[0], drc=len(rest) > 1 and rest[1].lower() == "drc"))
        elif word == "run":
            w.run(rest[0], rest[1:])
        elif word == "expect-ping":
            w.close()
            w.connect(wait=float(rest[0]) if rest else 60)
        else:
            print("   ", w.command(line))


def main():
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    p.add_argument("--host", default=os.environ.get("WIIU"), help="the console's address (or $WIIU)")
    sub = p.add_subparsers(dest="cmd", required=True)
    sub.add_parser("ping")
    sub.add_parser("info")
    s = sub.add_parser("shot")
    s.add_argument("file")
    s.add_argument("--drc", action="store_true", help="the GamePad's screen instead of the TV's")
    s.add_argument("--scale", type=int, default=1, help="keep one pixel in N each way")
    for name in ("press", "hold", "release"):
        s = sub.add_parser(name)
        s.add_argument("buttons", nargs="?", default="ALL")
        if name == "press":
            s.add_argument("ms", nargs="?", type=int, default=150)
    s = sub.add_parser("stick")
    s.add_argument("which", choices=["L", "R"])
    s.add_argument("x", type=float)
    s.add_argument("y", type=float)
    s.add_argument("ms", nargs="?", type=int, default=0)
    s = sub.add_parser("touch")
    s.add_argument("x", type=int)
    s.add_argument("y", type=int)
    s.add_argument("ms", nargs="?", type=int, default=150)
    sub.add_parser("clear")
    sub.add_parser("menu")
    s = sub.add_parser("launch")
    s.add_argument("title_id")
    s = sub.add_parser("runsd", help="start an .rpx/.wuhb on the SD card")
    s.add_argument("path", help="from the card's root, e.g. wiiu/apps/horizon/horizon-real.rpx")
    s = sub.add_parser("deploy", help="copy an .rpx/.wuhb to the SD card over FTP and start it")
    s.add_argument("file")
    s.add_argument("--dir", default="wiiu/apps/horizon")
    s = sub.add_parser("fetch", help="copy a file from the SD card")
    s.add_argument("remote")
    s.add_argument("local")
    sub.add_parser("reboot")
    sub.add_parser("shutdown")
    s = sub.add_parser("run")
    s.add_argument("file")
    s.add_argument("args", nargs="*")
    s = sub.add_parser("logs")
    s.add_argument("--seconds", type=float)
    s = sub.add_parser("install")
    s.add_argument("file", help="a .wps plugin (copied to Aroma's plugin folder)")
    s.add_argument("--no-reboot", action="store_true")
    s = sub.add_parser("watch", help="a screenshot every few seconds into a folder")
    s.add_argument("folder")
    s.add_argument("--every", type=float, default=5.0)
    s.add_argument("--drc", action="store_true")
    s = sub.add_parser("script")
    s.add_argument("file")
    a = p.parse_args()

    if a.cmd == "logs":
        logs(a.seconds)
        return
    if not a.host:
        p.error("no console address: --host or $WIIU")
    w = WiiU(a.host)
    if a.cmd == "ping":
        print(w.connect(), w.ping())
    elif a.cmd == "info":
        print(w.info())
    elif a.cmd == "shot":
        print(w.shot(a.file, a.drc, a.scale))
    elif a.cmd == "press":
        w.press(a.buttons, a.ms)
    elif a.cmd == "hold":
        w.hold(a.buttons)
    elif a.cmd == "release":
        w.release(a.buttons)
    elif a.cmd == "stick":
        w.stick(a.which, a.x, a.y, a.ms)
    elif a.cmd == "touch":
        w.touch(a.x, a.y, a.ms)
    elif a.cmd == "clear":
        w.clear()
    elif a.cmd == "menu":
        w.menu()
    elif a.cmd == "launch":
        w.launch(a.title_id)
    elif a.cmd == "runsd":
        print(w.runsd(a.path))
    elif a.cmd == "deploy":
        print(w.deploy(a.file, a.dir))
    elif a.cmd == "fetch":
        w.fetch(a.remote, a.local)
    elif a.cmd == "reboot":
        w.reboot()
    elif a.cmd == "shutdown":
        w.shutdown()
    elif a.cmd == "run":
        w.run(a.file, a.args)
    elif a.cmd == "install":
        w.upload(a.file, PLUGIN_DIR)
        print(f"copied to {PLUGIN_DIR}")
        if not a.no_reboot:
            try:
                w.reboot()
            except (OSError, BridgeError):
                print("reboot the console to load it (the bridge is not running yet)")
    elif a.cmd == "watch":
        os.makedirs(a.folder, exist_ok=True)
        n = 0
        while True:
            path = os.path.join(a.folder, f"{n:05d}.png")
            try:
                print(path, w.shot(path, a.drc, 2))
            except (OSError, BridgeError) as e:
                print("no picture:", e)
                w.close()
            n += 1
            time.sleep(a.every)
    elif a.cmd == "script":
        run_script(w, a.file)


if __name__ == "__main__":
    main()

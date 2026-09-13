"""Drive the Arbos desktop app from Python.

The app opens a Unix socket when it is started with ``ARBOS_DRIVER=1`` (or
``ARBOS_DRIVER_SOCKET=/path``). This module speaks that protocol: one JSON
request per line, one JSON reply per line. See ``desktop/src/driver.rs`` for
the server and the list of methods.

Standard library only, so a test needs nothing installed.

Quick use::

    from arbosdriver import Arbos

    with Arbos.launch() as app:
        app.hover("project-0")            # the "+" only mounts on hover
        app.click("project-add-0")        # new chat
        app.click("composer-field")
        app.type("open browser\\n")
        app.wait_state(lambda s: any(
            f["kind"] == "browser"
            for p in s["projects"] for f in p["surfaces"]), timeout=120)
        app.screenshot("/tmp/after.png")

Element names are gpui element ids. ``snapshot()["elements"]`` lists every
one on screen with its box; ``ids()`` lists just the names. A name can be the
last segment (``composer-field``), a dotted tail (``card.composer-field``), or
a pattern with ``*`` (``project-add-*``).

Coordinates are window points (logical pixels from the top-left of the
window's content). Screenshots are device pixels; ``hello()["window"]["scale"]``
is the ratio, 2.0 on a Retina display.
"""

from __future__ import annotations

import argparse
import fnmatch
import json
import os
import socket
import struct
import subprocess
import sys
import tempfile
import time
import zlib
from pathlib import Path
from typing import Any, Callable, Optional

HERE = Path(__file__).resolve().parent
DESKTOP = HERE.parent

Predicate = Callable[[Any], Any]


class DriverError(RuntimeError):
    """The app answered a request with ``ok: false``."""


class TimeoutError_(TimeoutError):
    """A ``wait_*`` ran out of time. Carries the last value seen."""

    def __init__(self, what: str, last: Any):
        super().__init__(f"timed out waiting for {what}")
        self.last = last


def find_binary() -> Path:
    """The desktop binary: ``$ARBOS_DESKTOP_BIN``, else the newest build."""
    env = os.environ.get("ARBOS_DESKTOP_BIN")
    if env:
        return Path(env)
    candidates = [
        DESKTOP / "target" / "debug" / "arbos-desktop",
        DESKTOP / "target" / "release" / "arbos-desktop",
    ]
    built = [path for path in candidates if path.exists()]
    if not built:
        raise FileNotFoundError(
            "no arbos-desktop binary; run `cargo build` in desktop/ "
            "or set ARBOS_DESKTOP_BIN"
        )
    return max(built, key=lambda path: path.stat().st_mtime)


def seed_state(xdg: Path, projects: list[str]) -> Path:
    """Write a minimal ``<xdg>/arbos-desktop/state.toml`` with these projects open."""
    folder = xdg / "arbos-desktop"
    folder.mkdir(parents=True, exist_ok=True)
    listed = ", ".join(json.dumps(str(Path(p).expanduser())) for p in projects)
    text = (
        f"projects = [{listed}]\n"
        "recents = []\n"
        "active = 0\n"
        'appearance = "dark"\n'
        "reduce_transparency = true\n"
        "cursor_blink = false\n"
        "text_size = 13.0\n"
        "bionic_reading = false\n"
        "hue = 0.0\n"
        "chroma = 0.0\n"
    )
    path = folder / "state.toml"
    path.write_text(text)
    return path


class Arbos:
    """One connection to a running desktop app."""

    def __init__(self, socket_path: str | os.PathLike, process: Optional[subprocess.Popen] = None):
        self.socket_path = Path(socket_path)
        self.process = process
        self._sock: Optional[socket.socket] = None
        self._reader = None
        self._next_id = 1
        # Which window requests go to: "main" (the chat window), "settings",
        # or an id from ``windows()``. ``use_window`` switches it.
        self.window: str = "main"

    def use_window(self, window: str) -> "Arbos":
        self.window = window
        return self

    def windows(self) -> list[dict]:
        """Every open window: kind ("main", "settings", "other"), id, size."""
        return self.call("windows")

    # -- lifecycle ---------------------------------------------------------

    @classmethod
    def launch(
        cls,
        binary: str | os.PathLike | None = None,
        socket_path: str | os.PathLike | None = None,
        env: Optional[dict] = None,
        timeout: float = 60.0,
        log: str | os.PathLike | None = None,
        xdg: str | os.PathLike | None = None,
        projects: Optional[list[str]] = None,
    ) -> "Arbos":
        """Start the app with the driver on and connect to it.

        ``timeout`` is how long to wait for the window to come up. ``log``
        is a file to send the app's stdout/stderr to; default is inherited.

        ``xdg`` makes the run private: it becomes ``XDG_CONFIG_HOME`` and
        ``XDG_DATA_HOME``, so the app reads and writes
        ``<xdg>/arbos-desktop/state.toml`` instead of the user's, and the
        kernel it spawns reads ``<xdg>/arbos/config.toml``. ``projects`` seeds
        that state with open project folders (the first one is active).
        """
        binary = Path(binary) if binary else find_binary()
        if socket_path is None:
            socket_path = Path(tempfile.gettempdir()) / f"arbos-driver-{os.getpid()}-{int(time.time())}.sock"
        socket_path = Path(socket_path)
        if socket_path.exists():
            socket_path.unlink()
        full_env = dict(os.environ)
        full_env["ARBOS_DRIVER_SOCKET"] = str(socket_path)
        if xdg is not None:
            xdg = Path(xdg)
            seed_state(xdg, projects or [])
            full_env["XDG_CONFIG_HOME"] = str(xdg)
            full_env["XDG_DATA_HOME"] = str(xdg / "data")
        if env:
            full_env.update(env)
        out = open(log, "ab") if log else None
        process = subprocess.Popen(
            [str(binary)],
            env=full_env,
            stdout=out,
            stderr=subprocess.STDOUT if out else None,
            cwd=str(DESKTOP),
        )
        app = cls(socket_path, process)
        try:
            app.connect(timeout)
        except Exception:
            app.close()
            raise
        return app

    def connect(self, timeout: float = 30.0) -> "Arbos":
        """Connect to the socket, waiting up to ``timeout`` for it to appear."""
        deadline = time.monotonic() + timeout
        last_err: Optional[Exception] = None
        while time.monotonic() < deadline:
            if self.process is not None and self.process.poll() is not None:
                raise RuntimeError(f"arbos-desktop exited with {self.process.returncode} before the driver came up")
            if self.socket_path.exists():
                try:
                    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                    sock.settimeout(120.0)
                    sock.connect(str(self.socket_path))
                    self._sock = sock
                    self._reader = sock.makefile("rb")
                    self.hello()
                    return self
                except (OSError, DriverError) as err:
                    last_err = err
                    self._drop()
            time.sleep(0.1)
        raise TimeoutError_(f"driver socket {self.socket_path}", last_err)

    def _drop(self) -> None:
        if self._reader is not None:
            try:
                self._reader.close()
            except OSError:
                pass
            self._reader = None
        if self._sock is not None:
            try:
                self._sock.close()
            except OSError:
                pass
            self._sock = None

    def close(self, quit_app: bool = True) -> None:
        """Quit the app (if we started it) and drop the connection."""
        if self.process is not None and quit_app and self._sock is not None:
            self.quit()
        self._drop()
        if self.process is not None:
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)
            self.process = None
        if self.socket_path.exists():
            try:
                self.socket_path.unlink()
            except OSError:
                pass

    def __enter__(self) -> "Arbos":
        return self

    def __exit__(self, *exc) -> None:
        self.close()

    # -- protocol ----------------------------------------------------------

    def call(self, method: str, **params: Any) -> Any:
        """Send one request and return its ``result``. Raises ``DriverError``."""
        if self._sock is None or self._reader is None:
            raise RuntimeError("not connected")
        request_id = self._next_id
        self._next_id += 1
        clean = {key: value for key, value in params.items() if value is not None}
        if self.window != "main" and method not in ("windows", "quit"):
            clean.setdefault("window", self.window)
        line = json.dumps({"id": request_id, "method": method, "params": clean}) + "\n"
        self._sock.sendall(line.encode("utf-8"))
        raw = self._reader.readline()
        if not raw:
            raise ConnectionError("driver closed the connection")
        reply = json.loads(raw)
        if not reply.get("ok"):
            raise DriverError(f"{method}: {reply.get('error', 'unknown error')}")
        return reply.get("result")

    # -- reading -----------------------------------------------------------

    def hello(self) -> dict:
        return self.call("hello")

    def snapshot(self) -> dict:
        """Every element on screen plus ``state``."""
        return self.call("snapshot")

    def state(self) -> dict:
        """The app's own account: pane, projects, sessions, surfaces, composer."""
        return self.call("state")

    def find(self, target: str) -> dict:
        """One element by name. Raises ``DriverError`` if none matches."""
        return self.call("find", target=target)

    def exists(self, target: str) -> bool:
        try:
            self.find(target)
            return True
        except DriverError:
            return False

    def elements(self, pattern: Optional[str] = None) -> list[dict]:
        """Elements on screen, optionally only those whose path matches."""
        found = self.snapshot()["elements"]
        if pattern is None:
            return found
        return [el for el in found if _matches(pattern, el["path"])]

    def ids(self) -> list[str]:
        """Every element path on screen, in draw order."""
        return [el["path"] for el in self.snapshot()["elements"]]

    # -- mouse -------------------------------------------------------------

    def hover(self, target: Optional[str] = None, x: Optional[float] = None, y: Optional[float] = None, **mods) -> dict:
        return self.call("move", target=target, x=x, y=y, modifiers=_mods(mods))

    move = hover

    def click(
        self,
        target: Optional[str] = None,
        x: Optional[float] = None,
        y: Optional[float] = None,
        button: str = "left",
        count: int = 1,
        **mods,
    ) -> dict:
        """Click an element by name, or a point. ``count=2`` double-clicks.

        A named target is hovered first, in its own settled frame: controls
        that mount or enable on hover (transcript copy/replay buttons, the
        project "+") need the app to render the hover before the press.
        """
        if target is not None:
            self.call("move", target=target, modifiers=_mods(mods))
        return self.call("click", target=target, x=x, y=y, button=button, count=count, modifiers=_mods(mods))

    def double_click(self, target: Optional[str] = None, x: Optional[float] = None, y: Optional[float] = None, **mods) -> dict:
        return self.click(target, x, y, count=2, **mods)

    def right_click(self, target: Optional[str] = None, x: Optional[float] = None, y: Optional[float] = None, **mods) -> dict:
        return self.click(target, x, y, button="right", **mods)

    def mouse_down(self, target: Optional[str] = None, x: Optional[float] = None, y: Optional[float] = None, button: str = "left", **mods) -> dict:
        return self.call("down", target=target, x=x, y=y, button=button, modifiers=_mods(mods))

    def mouse_up(self, target: Optional[str] = None, x: Optional[float] = None, y: Optional[float] = None, button: str = "left", **mods) -> dict:
        return self.call("up", target=target, x=x, y=y, button=button, modifiers=_mods(mods))

    def drag(self, start: str | tuple, end: str | tuple, steps: int = 8, button: str = "left", **mods) -> dict:
        """Press at ``start``, move in ``steps`` to ``end``, release.

        Each end is an element name or an ``(x, y)`` tuple.
        """
        return self.call("drag", **{"from": _where(start), "to": _where(end)}, steps=steps, button=button, modifiers=_mods(mods))

    def scroll(self, target: Optional[str] = None, x: Optional[float] = None, y: Optional[float] = None, dx: float = 0, dy: float = 0, lines: bool = False, **mods) -> dict:
        """Scroll at a point. ``dy`` negative scrolls content up (like a wheel)."""
        return self.call("scroll", target=target, x=x, y=y, dx=dx, dy=dy, lines=lines, modifiers=_mods(mods))

    # -- keyboard ----------------------------------------------------------

    def key(self, keys: str) -> dict:
        """Press chords, space-separated: ``"cmd-n"``, ``"enter"``, ``"cmd-shift-z escape"``."""
        return self.call("key", keys=keys)

    def type(self, text: str) -> dict:
        """Type text as keystrokes. ``\\n`` presses Enter, ``\\t`` presses Tab."""
        return self.call("type", text=text)

    def fill(self, target: str, text: str) -> dict:
        """Click a field, select all, and type ``text`` in its place."""
        return self.call("fill", target=target, text=text)

    # -- app ---------------------------------------------------------------

    def action(self, name: str, data: Any = None) -> dict:
        """Dispatch a gpui action by name, e.g. ``"arbos::NewSession"``."""
        return self.call("action", name=name, data=data)

    def resize(self, width: float, height: float) -> dict:
        return self.call("resize", width=width, height=height)

    def screenshot(self, path: str | os.PathLike | None = None) -> Path:
        """Capture the window to a PNG and return its path."""
        result = self.call("screenshot", path=str(path) if path else None)
        self.last_window_id = result.get("window_id")
        return Path(result["path"])

    def quit(self) -> None:
        """Ask the app to exit. The socket usually closes before a reply."""
        try:
            self.call("quit")
        except (ConnectionError, OSError):
            pass
        finally:
            self._drop()

    # -- waiting -----------------------------------------------------------

    def wait_for(self, read: Callable[[], Any], ok: Predicate, timeout: float = 30.0, every: float = 0.25, what: str = "condition") -> Any:
        """Poll ``read()`` until ``ok(value)`` is truthy. Returns the value."""
        deadline = time.monotonic() + timeout
        last = None
        while True:
            last = read()
            if ok(last):
                return last
            if time.monotonic() >= deadline:
                raise TimeoutError_(what, last)
            time.sleep(every)

    def wait_state(self, ok: Predicate, timeout: float = 30.0, every: float = 0.25, what: str = "state") -> dict:
        return self.wait_for(self.state, ok, timeout, every, what)

    def wait_element(self, target: str, timeout: float = 10.0, every: float = 0.1, reachable: bool = False) -> dict:
        """Wait until an element named ``target`` is on screen (and clickable, if asked)."""

        def read() -> Optional[dict]:
            try:
                return self.find(target)
            except DriverError:
                return None

        return self.wait_for(read, lambda el: el is not None and (el["reachable"] or not reachable), timeout, every, f"element `{target}`")

    def wait_gone(self, target: str, timeout: float = 10.0, every: float = 0.1) -> None:
        self.wait_for(lambda: self.exists(target), lambda present: not present, timeout, every, f"`{target}` to go away")

    def wait_idle(self, timeout: float = 120.0, every: float = 0.5) -> dict:
        """Wait until no session is streaming or mid-turn."""
        return self.wait_state(
            lambda s: not any(c["streaming"] or c["turn_open"] for p in s["projects"] for c in p["sessions"]),
            timeout,
            every,
            "the agent to go idle",
        )

    # -- convenience views of state ---------------------------------------

    def sessions(self) -> list[dict]:
        return [c for p in self.state()["projects"] for c in p["sessions"]]

    def surfaces(self, kind: Optional[str] = None) -> list[dict]:
        found = [f for p in self.state()["projects"] for f in p["surfaces"]]
        return [f for f in found if kind is None or f["kind"] == kind]

    def active_session(self) -> Optional[dict]:
        state = self.state()
        wanted = state.get("active_session")
        for project in state["projects"]:
            for chat in project["sessions"]:
                if chat["id"] == wanted:
                    return chat
        return None


def _mods(mods: dict) -> Optional[dict]:
    """Keyword modifiers (``shift=True``, ``cmd=True``…) to the wire shape."""
    known = {"shift", "ctrl", "alt", "cmd", "fn"}
    unknown = set(mods) - known
    if unknown:
        raise TypeError(f"unknown modifiers: {sorted(unknown)}")
    return {key: bool(value) for key, value in mods.items()} or None


def _where(spot: str | tuple) -> dict:
    if isinstance(spot, str):
        return {"target": spot}
    x, y = spot
    return {"x": float(x), "y": float(y)}


def _matches(pattern: str, path: str) -> bool:
    """Same rule as the server: full path, dotted tail, or ``*`` wildcards."""
    if "*" in pattern:
        return fnmatch.fnmatchcase(path, pattern) or fnmatch.fnmatchcase(path, "*." + pattern)
    return path == pattern or path.endswith("." + pattern)


# ---------------------------------------------------------------------------
# PNG reading, for checks on screenshots without any extra packages.


class Png:
    """An 8-bit RGB or RGBA PNG in memory. ``px(x, y)`` gives ``(r, g, b, a)``."""

    def __init__(self, path: str | os.PathLike):
        data = Path(path).read_bytes()
        if data[:8] != b"\x89PNG\r\n\x1a\n":
            raise ValueError("not a PNG")
        chunks = []
        pos = 8
        width = height = 0
        depth = color = interlace = 0
        while pos < len(data):
            (length,) = struct.unpack(">I", data[pos : pos + 4])
            kind = data[pos + 4 : pos + 8]
            body = data[pos + 8 : pos + 8 + length]
            pos += 12 + length
            if kind == b"IHDR":
                width, height, depth, color, _, _, interlace = struct.unpack(">IIBBBBB", body)
            elif kind == b"IDAT":
                chunks.append(body)
            elif kind == b"IEND":
                break
        if depth != 8 or color not in (2, 6) or interlace != 0:
            raise ValueError(f"unsupported PNG (depth={depth}, color={color}, interlace={interlace})")
        self.width, self.height = width, height
        self.channels = 3 if color == 2 else 4
        self._rows = _unfilter(zlib.decompress(b"".join(chunks)), width, height, self.channels)

    def px(self, x: int, y: int) -> tuple[int, int, int, int]:
        row = self._rows[y]
        off = x * self.channels
        r, g, b = row[off], row[off + 1], row[off + 2]
        a = row[off + 3] if self.channels == 4 else 255
        return r, g, b, a

    def mean(self, x: int, y: int, w: int, h: int, step: int = 4) -> tuple[float, float, float]:
        """Average colour over a box, sampling every ``step`` pixels."""
        total = [0, 0, 0]
        count = 0
        for yy in range(y, min(y + h, self.height), step):
            row = self._rows[yy]
            for xx in range(x, min(x + w, self.width), step):
                off = xx * self.channels
                total[0] += row[off]
                total[1] += row[off + 1]
                total[2] += row[off + 2]
                count += 1
        if not count:
            return (0.0, 0.0, 0.0)
        return tuple(value / count for value in total)  # type: ignore[return-value]

    def spread(self, x: int, y: int, w: int, h: int, step: int = 4) -> float:
        """How varied a box is: mean absolute distance from its mean colour.

        Near 0 means one flat colour (an empty panel); higher means content.
        """
        mr, mg, mb = self.mean(x, y, w, h, step)
        total = 0.0
        count = 0
        for yy in range(y, min(y + h, self.height), step):
            row = self._rows[yy]
            for xx in range(x, min(x + w, self.width), step):
                off = xx * self.channels
                total += abs(row[off] - mr) + abs(row[off + 1] - mg) + abs(row[off + 2] - mb)
                count += 1
        return total / (3 * count) if count else 0.0


def _unfilter(raw: bytes, width: int, height: int, channels: int) -> list[bytearray]:
    stride = width * channels
    rows: list[bytearray] = []
    prev = bytearray(stride)
    pos = 0
    for _ in range(height):
        kind = raw[pos]
        pos += 1
        cur = bytearray(raw[pos : pos + stride])
        pos += stride
        if kind == 1:
            for i in range(channels, stride):
                cur[i] = (cur[i] + cur[i - channels]) & 0xFF
        elif kind == 2:
            for i in range(stride):
                cur[i] = (cur[i] + prev[i]) & 0xFF
        elif kind == 3:
            for i in range(stride):
                left = cur[i - channels] if i >= channels else 0
                cur[i] = (cur[i] + ((left + prev[i]) >> 1)) & 0xFF
        elif kind == 4:
            for i in range(stride):
                a = cur[i - channels] if i >= channels else 0
                b = prev[i]
                c = prev[i - channels] if i >= channels else 0
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                pred = a if (pa <= pb and pa <= pc) else (b if pb <= pc else c)
                cur[i] = (cur[i] + pred) & 0xFF
        rows.append(cur)
        prev = cur
    return rows


def element_box_in_png(el: dict, scale: float) -> tuple[int, int, int, int]:
    """An element's box in screenshot pixels: ``(x, y, w, h)``."""
    return (int(el["x"] * scale), int(el["y"] * scale), int(el["w"] * scale), int(el["h"] * scale))


# ---------------------------------------------------------------------------
# Command line


def _cli(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        prog="arbosdriver",
        description="Talk to a running Arbos desktop, or launch one. "
        "Set ARBOS_DRIVER_SOCKET or pass --socket to reach an app already running.",
    )
    parser.add_argument("--socket", default=os.environ.get("ARBOS_DRIVER_SOCKET"), help="driver socket path")
    sub = parser.add_subparsers(dest="cmd", required=True)

    launch = sub.add_parser("launch", help="start the app with the driver on; prints the socket path")
    launch.add_argument("--binary", default=None)
    launch.add_argument("--log", default=None, help="file for the app's output")

    sub.add_parser("hello")
    sub.add_parser("state")
    sub.add_parser("snapshot")
    sub.add_parser("ids", help="list element paths on screen")
    find = sub.add_parser("find")
    find.add_argument("target")
    click = sub.add_parser("click")
    click.add_argument("target", nargs="?")
    click.add_argument("--x", type=float)
    click.add_argument("--y", type=float)
    click.add_argument("--button", default="left")
    click.add_argument("--count", type=int, default=1)
    hover = sub.add_parser("hover")
    hover.add_argument("target", nargs="?")
    hover.add_argument("--x", type=float)
    hover.add_argument("--y", type=float)
    typ = sub.add_parser("type")
    typ.add_argument("text")
    fill = sub.add_parser("fill")
    fill.add_argument("target")
    fill.add_argument("text")
    key = sub.add_parser("key")
    key.add_argument("keys")
    action = sub.add_parser("action")
    action.add_argument("name")
    shot = sub.add_parser("screenshot")
    shot.add_argument("path", nargs="?")
    resize = sub.add_parser("resize")
    resize.add_argument("width", type=float)
    resize.add_argument("height", type=float)
    sub.add_parser("quit")
    raw = sub.add_parser("call", help="any method with JSON params")
    raw.add_argument("method")
    raw.add_argument("params", nargs="?", default="{}")

    args = parser.parse_args(argv)

    if args.cmd == "launch":
        socket_path = Path(tempfile.gettempdir()) / f"arbos-driver-{int(time.time())}.sock"
        binary = Path(args.binary) if args.binary else find_binary()
        env = dict(os.environ, ARBOS_DRIVER_SOCKET=str(socket_path))
        out = open(args.log, "ab") if args.log else None
        subprocess.Popen([str(binary)], env=env, stdout=out, stderr=subprocess.STDOUT if out else None, cwd=str(DESKTOP), start_new_session=True)
        app = Arbos(socket_path)
        app.connect(60)
        app._drop()
        print(socket_path)
        print(f"export ARBOS_DRIVER_SOCKET={socket_path}", file=sys.stderr)
        return 0

    if not args.socket:
        parser.error("no socket: pass --socket or set ARBOS_DRIVER_SOCKET (or use `launch`)")
    app = Arbos(args.socket)
    app.connect(5)

    if args.cmd == "hello":
        out = app.hello()
    elif args.cmd == "state":
        out = app.state()
    elif args.cmd == "snapshot":
        out = app.snapshot()
    elif args.cmd == "ids":
        for path in app.ids():
            print(path)
        return 0
    elif args.cmd == "find":
        out = app.find(args.target)
    elif args.cmd == "click":
        out = app.click(args.target, args.x, args.y, button=args.button, count=args.count)
    elif args.cmd == "hover":
        out = app.hover(args.target, args.x, args.y)
    elif args.cmd == "type":
        out = app.type(args.text.encode("utf-8").decode("unicode_escape"))
    elif args.cmd == "fill":
        out = app.fill(args.target, args.text)
    elif args.cmd == "key":
        out = app.key(args.keys)
    elif args.cmd == "action":
        out = app.action(args.name)
    elif args.cmd == "screenshot":
        out = str(app.screenshot(args.path))
    elif args.cmd == "resize":
        out = app.resize(args.width, args.height)
    elif args.cmd == "quit":
        app.quit()
        return 0
    elif args.cmd == "call":
        out = app.call(args.method, **json.loads(args.params))
    else:
        parser.error(f"unknown command {args.cmd}")
    print(json.dumps(out, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(_cli(sys.argv[1:]))

#!/usr/bin/env python3
"""Move the relay's Docker `runsc` (gvisor) boxes onto arbos-boxd, in place.

The procedure is crates/arbos-boxd/README.md "Moving a Docker runsc box here", one box at a
time, as one command that can be stopped and run again:

  idle      the box is awake, answers, and has had no turn, waiting send or job for 30 s
  link      the daemon gets the same work.img under its own name (a hard link: no copy, and
            from here on the daemon never finds this user without a disk)
  switch    the user's next box goes to boxd, and the Docker box is removed; its disk stays
  start     the box is made on the daemon, on the linked disk, and woken
  verify    it is awake on boxd, agentd answers as this user, the daemon runs it, and the
            disk under the daemon's name is the same file and the same filesystem as before
  finish    only now the old name is removed

A box that does not verify is put back: off the daemon, onto gvisor, on the disk it had, the
daemon's link removed, and woken there. The old name is never removed before `verify` passes,
and the daemon's link is never removed unless the old name still names the same file, so the
disk always has a name. Every box ends fully on one provider or the other.

Where a box stands is read from the host and the relay each time, plus one small record per
box under --state-dir (what its disk was before the move), so a second run picks every box up
where the first left it and does nothing to a box that is done.

    boxd-migrate.py --dry-run (--all | --user ID ...)    print what would be done; change nothing
    boxd-migrate.py --yes     (--all | --user ID ...)    do it

Run as root on the host that has both the relay's data and the daemon. Needs
ARBOS_RELAY_ADMIN_TOKEN in the environment. Exit 0: every box named is on boxd or was left
alone for a stated reason. 1: a box was rolled back, or is stuck and needs a person. 2: it did
not run.
"""
import argparse
import fcntl
import json
import os
import re
import shutil
import socket
import stat
import subprocess
import sys
import time
import urllib.error
import urllib.request
import uuid
from collections import namedtuple

GIB = 1 << 30
TIERS = (8 * GIB, 32 * GIB, 128 * GIB)  # the sizes the relay takes a boxd disk in
USER_ID = re.compile(r"^[A-Za-z0-9_-]{1,64}$")  # arbos-boxd's `UserId::parse`

# What one look at one box found. `old`/`new` are (st_dev, st_ino) of the work.img under the
# relay's name and the daemon's, or None; `record` is this tool's note from an earlier run.
Seen = namedtuple("Seen", "provider state old new size same_fs installed record")


def decide(s):
    """The next step for a box, from what was seen of it: (step, why). Pure.

    Steps: link, switch, start, verify, done, skip (not this tool's to touch, with the reason),
    stuck (a state no run of this tool leaves; a person looks).
    """
    linked = s.old is not None and s.old == s.new
    if s.provider == "gvisor":
        if s.old is None:
            return "skip", "no work.img under the relay's name"
        if s.new is not None and not linked:
            return "stuck", "the daemon already holds a different disk for this user"
        if linked:
            return "switch", "linked; the Docker box is still there"
        if s.size not in TIERS:
            return "skip", f"work.img is {s.size / GIB:g} GiB; grow it to 8, 32 or 128 first"
        if not s.same_fs:
            return "skip", "the relay's data and the daemon's root are on different filesystems"
        if not s.installed:
            return "skip", "its image version is not installed on the daemon"
        if s.state != "awake":
            return "skip", f"the box is {s.state}, not awake: it is checked before it is moved"
        return "link", "idle check, then link"
    if s.provider is None:
        if linked and s.record:
            return "start", "linked and off Docker; not yet on the daemon"
        return "skip", "no box"
    if s.provider == "boxd":
        if linked:
            return "verify", "on the daemon; the old name is still there"
        if s.new is not None and s.old is None:
            return "done", "on the daemon"
        if s.new is None:
            return "skip", "a boxd box with no disk on this host"
        return "skip", "on the daemon with a disk of its own; the old work.img is not it"
    return "skip", f"provider {s.provider}"


def checks(before, after):
    """Why a box on the daemon is not the box that was moved, or None if it is. Pure.

    `before` is the record written at the link; `after` is what `World.after` saw.
    """
    if after["state"] != "awake" or after["provider"] != "boxd" or not after["node"]:
        return f"the relay has it {after['state']} on {after['provider']}"
    if not after["answers"]:
        return "agentd does not answer"
    if after["box_user"] != before["user"]:
        return "the box that answers is another user's"
    if after["running"] is None:
        return "the daemon does not answer"
    if not after["running"]:
        return "the daemon does not list it as running"
    if after["file"] != before["file"]:
        return "the daemon's work.img is not the file that was linked"
    if after["fs_uuid"] != before["fs_uuid"]:
        return "the filesystem in the daemon's work.img is not the one that was linked"
    return None


def on_this_host(url):
    """Is this plain HTTP to an address of this host: loopback, or one an interface here has?

    The kernel routes to one of its own addresses from that same address and to anyone else's
    from another, so a UDP socket pointed there says which it is; no packet is sent.
    """
    if re.match(r"^http://(127\.0\.0\.1|\[::1\]):\d+$", url):
        return True
    ip = re.match(r"^http://(\d+\.\d+\.\d+\.\d+):\d+$", url)
    if not ip:
        return False
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as s:
        try:
            s.connect((ip[1], 9))
            return s.getsockname()[0] == ip[1]
        except OSError:
            return False


def idle(box):
    """Why a box is not idle, or None. `box` is `GET /machine/debug?user=`. Pure."""
    relay, on_box = box.get("relay") or {}, box.get("box") or {}
    if relay.get("state") != "awake":
        return f"the box is {relay.get('state')}"
    if not on_box.get("reachable"):
        return "agentd does not answer"
    if relay.get("turn_running_since"):
        return "a turn is running"
    for w in (on_box.get("state") or {}).get("workspaces") or []:
        turn = w.get("turn") or {}
        if turn.get("running") or turn.get("sends_waiting") or w.get("jobs_running"):
            return f"workspace {w.get('project_id')} is busy"
    return None


class World:
    """The relay, the daemon and the host's files. Everything that has an effect is here."""

    def __init__(self, args, admin_token):
        self.a, self.admin = args, admin_token
        mode = os.lstat(args.boxd_token_file).st_mode
        if not stat.S_ISREG(mode) or mode & 0o077:
            raise SystemExit(f"{args.boxd_token_file} must be a plain file with mode 0600")
        with open(args.boxd_token_file) as fh:
            self.boxd_token = fh.read().strip()

    # -- paths -------------------------------------------------------------------------------
    def old(self, user):
        return os.path.join(self.a.data, "boxes", user, "work.img")

    def new_dir(self, user):
        return os.path.join(self.a.boxd_root, "boxes", user)

    def new(self, user):
        return os.path.join(self.new_dir(user), "work.img")

    def record_path(self, user):
        return os.path.join(self.a.state_dir, f"{user}.json")

    # -- http --------------------------------------------------------------------------------
    def _http(self, url, token, method="GET", body=None, timeout=60):
        data = None if body is None else json.dumps(body).encode()
        headers = {"Authorization": f"Bearer {token}", "content-type": "application/json"}
        req = urllib.request.Request(url, data=data, method=method, headers=headers)
        with urllib.request.urlopen(req, timeout=timeout) as r:
            raw = r.read()
            return json.loads(raw) if raw.strip() else None

    def relay(self, method, path, body=None, token=None, timeout=60):
        return self._http(f"{self.a.relay}{path}", token or self.admin, method, body, timeout)

    def users(self):
        """user id -> the `machine` block of GET /admin/users."""
        return {u["id"]: u.get("machine") or {} for u in self.relay("GET", "/admin/users")}

    def debug(self, user):
        return self.relay("GET", f"/machine/debug?user={user}")

    def daemon_boxes(self):
        host = self._http(f"{self.a.boxd_url}/v1/host/status", self.boxd_token)
        return {b["user_id"]: b for b in host["boxes"]}

    # -- looking -----------------------------------------------------------------------------
    @staticmethod
    def file_id(path):
        try:
            st = os.lstat(path)
        except FileNotFoundError:
            return None, 0
        if not stat.S_ISREG(st.st_mode):
            raise RuntimeError(f"{path} is not a plain file")
        return (st.st_dev, st.st_ino), st.st_size

    @staticmethod
    def fs_uuid(path):
        """The ext4 filesystem's own id, from its superblock: the same before and after any
        mount, which a hash of the image's bytes is not."""
        with open(path, "rb") as fh:
            fh.seek(1024)
            sb = fh.read(1024)
        if len(sb) < 0x78 or sb[0x38:0x3A] != b"\x53\xef":
            raise RuntimeError(f"{path} does not hold an ext4 filesystem")
        return str(uuid.UUID(bytes=sb[0x68:0x78]))

    def record(self, user):
        try:
            with open(self.record_path(user)) as fh:
                return json.load(fh)
        except FileNotFoundError:
            return None

    def write_record(self, user, **fields):
        """Whole or not at all, and on the disk before the step it describes."""
        rec = {**(self.record(user) or {}), **fields, "user": user, "at": int(time.time())}
        path = self.record_path(user)
        with open(f"{path}.tmp", "w", opener=lambda p, f: os.open(p, f, 0o600)) as fh:
            json.dump(rec, fh)
            fh.flush()
            os.fsync(fh.fileno())
        os.rename(f"{path}.tmp", path)
        return rec

    def seen(self, user, machine):
        old, size = self.file_id(self.old(user))
        new, _ = self.file_id(self.new(user))
        version = (machine.get("snapshot") or "").removeprefix("arbos-agent-")
        image = os.path.join(self.a.boxd_root, "images", version)
        installed = bool(version) and (
            os.path.isdir(os.path.join(image, "rootfs"))
            or os.path.isfile(os.path.join(image, "rootfs.tar"))
        )
        same_fs = old is not None and old[0] == os.stat(self.a.boxd_root).st_dev
        return Seen(
            provider=machine.get("provider") if machine.get("exists") else None,
            state=machine.get("state"),
            old=old, new=new, size=size, same_fs=same_fs, installed=installed,
            record=self.record(user),
        )

    def attached(self, path):
        """Is a loop device holding this file?"""
        out = subprocess.run(["losetup", "-j", path], capture_output=True, text=True, check=True)
        return bool(out.stdout.strip())

    def wait(self, what, check, seconds, bind=False):
        """Poll `check` until it returns None (good) or time is up; the last reason, or None.
        With `bind`, a good answer that comes after the time is up is not one."""
        deadline = time.monotonic() + seconds
        while True:
            why = check()
            late = time.monotonic() >= deadline
            if bind and late and why is None:
                why = f"not within {seconds:g} s"
            if why is None or late:
                return why and f"{what}: {why}"
            time.sleep(self.a.poll)

    def wait_idle(self, user):
        """None once the box has been idle for --idle-seconds on end, else why it never was."""
        quiet_since, deadline = None, time.monotonic() + self.a.idle_wait
        while True:
            why = idle(self.debug(user))
            now = time.monotonic()
            quiet_since = None if why else (quiet_since or now)
            if quiet_since is not None and now - quiet_since >= self.a.idle_seconds:
                return None
            if now >= deadline:
                return why or "it was not idle for long enough"
            time.sleep(self.a.poll)

    def after(self, user):
        box = self.debug(user)
        relay, on_box = box.get("relay") or {}, box.get("box") or {}
        provider = relay.get("provider") or {}
        file, _ = self.file_id(self.new(user))
        try:
            running = bool(self.daemon_boxes().get(user, {}).get("running"))
        except (urllib.error.URLError, OSError):
            running = None  # the daemon did not answer: not verified, and `checks` says so
        return {
            "state": relay.get("state"),
            "provider": provider.get("provider"),
            "node": provider.get("node_id"),
            "answers": bool(on_box.get("reachable")),
            "box_user": (on_box.get("state") or {}).get("user_id"),
            "running": running,
            "file": list(file) if file else None,
            "fs_uuid": self.fs_uuid(self.new(user)) if file else None,
        }

    # -- doing -------------------------------------------------------------------------------
    def link(self, user):
        os.makedirs(self.new_dir(user), mode=0o700, exist_ok=True)
        os.link(self.old(user), self.new(user))

    def set_provider(self, user, provider):
        self.relay("PUT", f"/admin/users/{user}/box", {"provider": provider})

    def delete_box(self, user):
        self.relay("DELETE", f"/admin/users/{user}/box", timeout=300)

    def start(self, user):
        """Wake the user's box the way their own app does, as a device that exists for this."""
        name = {"name": "boxd migration (revoked when the box is up)"}
        dev = self.relay("POST", f"/admin/users/{user}/devices", name)
        try:
            self.relay("POST", "/machine/start", token=dev["access_token"], timeout=300)
        finally:
            try:
                self.relay("DELETE", f"/auth/devices/{dev['device_id']}", token=dev["access_token"])
            except (urllib.error.URLError, OSError) as e:
                print(f"  {user}: the migration device {dev['device_id']} was not revoked: {e}")

    def unlink_old(self, user):
        os.unlink(self.old(user))

    def remove_new(self, user):
        shutil.rmtree(self.new_dir(user))


def plan(w, user):
    """What a real run would do to an untouched box, in order, for --dry-run."""
    a, old, new = w.a, w.old(user), w.new(user)
    return [
        f"wait until idle for {a.idle_seconds:g} s: GET {a.relay}/machine/debug?user={user}",
        f"write {w.record_path(user)} (the disk's file and filesystem ids)",
        f"mkdir -m 0700 {w.new_dir(user)} && ln {old} {new}",
        f'PUT {a.relay}/admin/users/{user}/box {{"provider": "boxd"}}',
        f"DELETE {a.relay}/admin/users/{user}/box, then wait until `losetup -j {old}` prints nothing",
        f"POST {a.relay}/machine/start as a device minted for {user}, revoked after",
        f"verify within {a.wake_seconds:g} s: awake on boxd, agentd answers as {user}, "
        f"{a.boxd_url}/v1/host/status lists it running, {new} is the linked file and filesystem",
        f"rm {old}",
        "on any failure before the rm: DELETE the box, provider back to gvisor, "
        f"rm -r {w.new_dir(user)} (only while {old} still names the file), POST /machine/start",
    ]


def rollback(w, user, why):
    """Put a box that did not verify back on gvisor, on the disk it had. Returns the outcome."""
    print(f"  {user}: rolling back: {why}")
    w.write_record(user, phase="rolling_back", why=why)
    # The provider first: whatever makes this user's next box makes it on gvisor.
    w.set_provider(user, "gvisor")
    machine = w.users().get(user) or {}
    if machine.get("exists") and machine.get("provider") == "boxd":
        try:
            w.delete_box(user)  # a forced stop on the daemon: nothing holds the disk after it
        except (urllib.error.URLError, OSError) as e:
            return "stuck", f"{why}; not rolled back: the relay did not take the box off the daemon: {e}"
    old, _ = w.file_id(w.old(user))
    new, _ = w.file_id(w.new(user))
    if new is not None:
        if old is None or old != new:
            # Never the last name of a disk. No run of this tool gets here.
            return "stuck", f"{why}; not rolled back: {w.old(user)} no longer names the daemon's disk"
        held = w.wait("the daemon still holds the disk", lambda: "attached" if w.attached(w.new(user)) else None, 60)
        if held:
            return "stuck", f"{why}; not rolled back: {held}"
        w.remove_new(user)
    try:
        w.start(user)
    except (urllib.error.URLError, OSError) as e:
        return "stuck", f"{why}; the disk is back under gvisor alone, but its start was refused: {e}"

    def back():
        m = w.users().get(user) or {}
        if m.get("provider") == "gvisor" and m.get("state") == "awake":
            return None
        return f"the box is {m.get('state')} on {m.get('provider')}"

    not_back = w.wait("the gvisor box did not come back", back, w.a.wake_seconds)
    if not_back:
        return "stuck", f"{why}; the disk is back under gvisor alone, but {not_back}"
    w.write_record(user, phase="rolled_back", why=why)
    return "rolled_back", why


def migrate(w, user):
    """Take one box as far as it goes. Returns (outcome, why): moved (by this run), done
    (before it), skip, rolled_back, stuck."""
    for turn in range(8):  # every step moves a box on, and there are four
        machine = w.users().get(user)
        if machine is None:
            return "skip", "no such user"
        s = w.seen(user, machine)
        step, why = decide(s)
        if step in ("done", "skip", "stuck"):
            return ("moved" if step == "done" and turn else step), why
        print(f"  {user}: {step}")
        if step == "link":
            busy = w.wait_idle(user)
            if busy:
                return "skip", f"not idle: {busy}"
            w.write_record(user, phase="linking", file=list(s.old),
                           fs_uuid=w.fs_uuid(w.old(user)), size=s.size)
            w.link(user)
        elif step == "switch":
            if s.record is None:
                return "stuck", "linked, but not by this tool: no record of the disk before"
            # The Docker box may have been used since the link: it is asked again.
            busy = w.wait_idle(user) if s.state == "awake" else None
            if busy:
                return "skip", f"not idle: {busy}; linked, run again later"
            w.write_record(user, phase="switching")
            w.set_provider(user, "boxd")
            w.delete_box(user)
            held = w.wait("Docker still holds the disk", lambda: "attached" if w.attached(w.old(user)) else None, 60)
            if held:
                return rollback(w, user, held)
        elif step == "start":
            w.write_record(user, phase="starting")
            try:
                w.start(user)
            except (urllib.error.URLError, OSError) as e:
                # The relay's own sweep may have made the box a moment before this asked:
                # then it is there, and `verify` is what judges it.
                machine = w.users().get(user) or {}
                if not (machine.get("exists") and machine.get("provider") == "boxd"):
                    return rollback(w, user, f"the start was refused: {e}")
        elif step == "verify":
            if s.record is None:
                return "skip", "on the daemon, but not by this tool; the old work.img is left alone"
            bad = w.wait("not verified", lambda: checks(s.record, w.after(user)), w.a.wake_seconds, bind=True)
            if bad:
                return rollback(w, user, bad)
            # The one step that cannot be taken back, and the last.
            w.unlink_old(user)
            w.write_record(user, phase="done")
    return "stuck", "it did not settle in eight steps"


def main(argv=None, world=World):
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--dry-run", action="store_true", help="print what would be done; change nothing")
    ap.add_argument("--yes", action="store_true", help="do it")
    ap.add_argument("--all", action="store_true", help="every gvisor box, and every box a run left part way")
    ap.add_argument("--user", action="append", default=[], metavar="ID")
    ap.add_argument("--limit", type=int, default=0, help="stop after this many boxes have been moved")
    ap.add_argument("--relay", default="http://127.0.0.1:7700", help="at an address of this host")
    ap.add_argument("--data", default="/var/lib/arbos-relay", help="the relay's data directory")
    ap.add_argument("--boxd-url", default="http://127.0.0.1:7790")
    ap.add_argument("--boxd-root", default="/var/lib/arbos-boxd")
    ap.add_argument("--boxd-token-file", default="/etc/arbos-boxd/token")
    ap.add_argument("--state-dir", default="/var/lib/arbos-boxd/migrate")
    ap.add_argument("--idle-seconds", type=float, default=30)
    ap.add_argument("--idle-wait", type=float, default=300, help="how long to wait for a box to go idle")
    ap.add_argument("--wake-seconds", type=float, default=300, help="how long a moved box has to verify")
    ap.add_argument("--poll", type=float, default=5)
    a = ap.parse_args(argv)
    a.relay, a.boxd_url = a.relay.rstrip("/"), a.boxd_url.rstrip("/")

    if a.dry_run == a.yes:
        print("boxd-migrate: name one of --dry-run (show what would be done) or --yes (do it)", file=sys.stderr)
        return 2
    if a.all == bool(a.user):
        print("boxd-migrate: name the boxes with --all or with --user ID", file=sys.stderr)
        return 2
    bad = [u for u in a.user if not USER_ID.match(u)]
    if bad:
        print(f"boxd-migrate: not a user id: {bad[0]!r}", file=sys.stderr)
        return 2
    # The admin token and the daemon's go to this host and nowhere else. The relay may be on
    # the bridge address, where a box can dial it.
    for url in (a.relay, a.boxd_url):
        if not url.startswith("http://"):
            print(f"boxd-migrate: {url} is not an http:// address; the relay and the daemon are asked in plain HTTP, on this host", file=sys.stderr)
            return 2
        if not on_this_host(url):
            print(f"boxd-migrate: {url} is not an address of this host; run this on the host", file=sys.stderr)
            return 2
    admin = os.environ.get("ARBOS_RELAY_ADMIN_TOKEN", "").strip()
    if not admin:
        print("boxd-migrate: ARBOS_RELAY_ADMIN_TOKEN is not set", file=sys.stderr)
        return 2

    w = world(a, admin)
    machines = w.users()
    users = a.user or sorted(
        u for u, m in machines.items()
        if (m.get("exists") and m.get("provider") == "gvisor") or w.record(u)
    )
    results = []
    if a.dry_run:
        for user in users:
            machine = machines.get(user)
            try:
                step, why = ("skip", "no such user") if machine is None else decide(w.seen(user, machine))
            except (OSError, RuntimeError) as e:
                step, why = "stuck", f"{type(e).__name__}: {e}"
            print(f"{user}: {step} ({why})")
            if step == "link":
                for line in plan(w, user):
                    print(f"    would {line}")
            elif step in ("switch", "start", "verify"):
                print(f"    would resume at `{step}` and go on from there as for a new box")
            results.append((user, f"would {step}", why))
    else:
        os.makedirs(a.state_dir, mode=0o700, exist_ok=True)
        lock = open(os.path.join(a.state_dir, "lock"), "w")
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            print("boxd-migrate: another run holds the lock", file=sys.stderr)
            return 2
        moved = 0
        for user in users:
            if a.limit and moved >= a.limit:
                results.append((user, "skip", "--limit reached"))
                continue
            print(f"{user}:")
            try:
                outcome, why = migrate(w, user)
            except (urllib.error.URLError, OSError, RuntimeError, subprocess.CalledProcessError, KeyError) as e:
                # Whatever the step was, the box is where `decide` will find it on the next run.
                outcome, why = "stuck", f"{type(e).__name__}: {e}"
            moved += outcome == "moved"
            results.append((user, outcome, why))

    print("\nsummary:")
    counts = {}
    for user, outcome, why in results:
        counts[outcome] = counts.get(outcome, 0) + 1
        print(f"  {outcome:<12} {user}  {why}")
    print("  " + ", ".join(f"{n} {o}" for o, n in sorted(counts.items())) if counts else "  no boxes")
    return 1 if any(o in ("rolled_back", "stuck") for _, o, _ in results) else 0


if __name__ == "__main__":
    sys.exit(main())

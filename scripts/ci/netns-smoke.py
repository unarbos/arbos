#!/usr/bin/env python3
"""Verify the real Linux test fence, using only a self-hosted loopback fixture."""

import errno
import json
import os
from pathlib import Path
import socket
import sys


def inspect_process(text, uid, gid):
    fields = dict(line.split(":", 1) for line in text.splitlines() if ":" in line)
    if uid <= 0 or gid <= 0:
        raise RuntimeError("non-root runner identity required")
    if list(map(int, fields["Uid"].split())) != [uid] * 4:
        raise RuntimeError("runner UID was not preserved")
    if list(map(int, fields["Gid"].split())) != [gid] * 4:
        raise RuntimeError("runner GID was not preserved")
    if fields["Groups"].split() or fields["NoNewPrivs"].strip() != "1":
        raise RuntimeError("groups or no-new-privs fence missing")
    if any(int(fields[name], 16) for name in ("CapInh", "CapPrm", "CapEff", "CapBnd", "CapAmb")):
        raise RuntimeError("capabilities remain")


def inspect_network(interfaces, routes, ipv6_routes):
    if [name for _, name in interfaces] != ["lo"]:
        raise RuntimeError("network namespace has an interface other than loopback")
    if any(row.split()[0] != "lo" for row in routes.splitlines()[1:] if row.strip()):
        raise RuntimeError("non-loopback IPv4 route exists")
    if any(row.split()[-1] != "lo" for row in ipv6_routes.splitlines() if row.strip()):
        raise RuntimeError("non-loopback IPv6 route exists")


def check_sockets():
    # Both peers belong to this process, in the new namespace. No DNS is used.
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as listener:
        listener.settimeout(1)
        listener.bind(("127.0.0.1", 0))
        listener.listen(1)
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as client:
            client.settimeout(1)
            client.connect(listener.getsockname())
            with listener.accept()[0] as accepted:
                accepted.settimeout(1)
                client.sendall(b"ci-loopback")
                if accepted.recv(32) != b"ci-loopback":
                    raise RuntimeError("loopback fixture did not work")

    # Only reached after proving there is no uplink. TEST-NET-1 is a numeric
    # documentation address; an unreachable result is local kernel routing, not
    # a request sent to an external service. Success or timeout fails the gate.
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as outside:
        outside.settimeout(0.2)
        result = outside.connect_ex(("192.0.2.1", 9))
    if result not in (errno.ENETUNREACH, errno.EHOSTUNREACH):
        raise RuntimeError("non-loopback socket was not immediately unreachable")
    return result


def run_checks(uid, gid):
    if sys.platform != "linux":
        raise RuntimeError("Linux kernel inspection is required")
    inspect_process(Path("/proc/self/status").read_text(), uid, gid)
    if os.access("/var/run/docker.sock", os.R_OK | os.W_OK):
        raise RuntimeError("host Docker socket remains accessible")
    interfaces = socket.if_nameindex()
    routes = Path("/proc/net/route").read_text()
    ipv6 = Path("/proc/net/ipv6_route")
    inspect_network(interfaces, routes, ipv6.read_text() if ipv6.exists() else "")
    result = check_sockets()
    return {"uid": uid, "gid": gid, "groups": [], "no_new_privs": 1,
            "capability_sets_zero": True, "interfaces": ["lo"],
            "loopback_fixture": "ok", "nonloopback_errno": result}


def main():
    args = sys.argv[1:]
    if len(args) != 8 or args[2] != "--" or args[4:] != ["test", "--workspace", "--locked", "--offline"]:
        raise RuntimeError("expected runner UID/GID and the exact offline workspace test command")
    uid, gid = int(args[0]), int(args[1])
    if not os.path.isabs(args[3]) or not os.access(args[3], os.X_OK):
        raise RuntimeError("an absolute executable Cargo path is required")
    print(json.dumps({"network_fence": run_checks(uid, gid)}), flush=True)
    os.execv(args[3], args[3:])


if __name__ == "__main__":
    try:
        main()
    except RuntimeError as error:
        print(f"network fence smoke failed: {error}", file=sys.stderr)
        sys.exit(1)
    except (KeyError, ValueError, OSError):
        print("network fence smoke could not verify the kernel state", file=sys.stderr)
        sys.exit(1)

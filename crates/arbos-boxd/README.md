# arbos-boxd

The root daemon on a compute host that runs each user's box as an OCI bundle directly under
`runsc` (docs/box-scaling.md §3 and §4.1). The relay calls it for boxes on its
`boxd` provider, which is off unless the relay has `ARBOS_BOXD_URL` and `ARBOS_BOXD_TOKEN_FILE`
(a copy of the token below that the relay's user can read, mode 0600). The R2 cold tier and
multi-node are later phases.

## What the host needs

- Linux, root, and on `PATH`: `runsc`, `mkfs.ext4` and `resize2fs` (e2fsprogs), `losetup`,
  `mount`, `umount`, `mountpoint` (util-linux), `ip` (iproute2), `tar`, `chown`.
- A token file only root reads. The daemon refuses to start on anything else:

  ```sh
  install -d -m 0700 /etc/arbos-boxd
  (umask 077; openssl rand -hex 32 > /etc/arbos-boxd/token)
  ```

- Outbound traffic for the boxes. The daemon makes the bridge `arbos-boxd0` (`10.207.0.1/16`)
  and one veth per awake box; forwarding and NAT are the host's, set once:

  ```sh
  sysctl -w net.ipv4.ip_forward=1
  iptables -t nat -A POSTROUTING -s 10.207.0.0/16 ! -o arbos-boxd0 -j MASQUERADE
  ```

  Where the firewall drops by default (ufw does: `FORWARD` and `INPUT` are `DROP`), those two
  are not enough: a box resolves no name and reaches nothing until its traffic may be
  forwarded, and it cannot reach a relay on this host until the bridge may come in on the
  relay's port:

  ```sh
  iptables -I FORWARD -i arbos-boxd0 ! -o arbos-boxd0 -s 10.207.0.0/16 -j ACCEPT
  iptables -I FORWARD -o arbos-boxd0 -d 10.207.0.0/16 -m conntrack --ctstate RELATED,ESTABLISHED -j ACCEPT
  iptables -I INPUT -i arbos-boxd0 -d 10.207.0.1 -p tcp --dport <relay port> -j ACCEPT
  ```

## Installing an image version

A box's root is `/var/lib/arbos-boxd/images/<version>/rootfs`, shared read-only by every box
on that version. The daemon unpacks it once, on the first `ensure` that names the version,
from two files the operator puts there:

```sh
v=0.6.409; image=ghcr.io/unarbos/arbos/arbos-agent:$v
dir=/var/lib/arbos-boxd/images/$v; mkdir -p "$dir"
c=$(docker create "$image") && docker export "$c" > "$dir/rootfs.tar" && docker rm "$c"
docker image inspect --format '{{json .Config}}' "$image" > "$dir/image.json"
```

`rootfs.tar` is the filesystem; `image.json` is what a tar does not carry (entrypoint,
environment, working directory). A `docker export` drops file capabilities, which the boxes
do not use. The image must have `/work`, `/home/daytona`, `/etc`, `/tmp`, `/run` and `/mnt`,
as `arbos-agent` does: runsc creates a missing mount point, and it would create it in the
shared rootfs.

## Running

Settings are environment variables, all optional:

| Variable | Default |
| --- | --- |
| `ARBOS_BOXD_ROOT` | `/var/lib/arbos-boxd` |
| `ARBOS_BOXD_RUNSC_ROOT` | `/var/run/arbos-runsc` |
| `ARBOS_BOXD_TOKEN_FILE` | `/etc/arbos-boxd/token` |
| `ARBOS_BOXD_PORT` | `7790` (always on `127.0.0.1`) |
| `ARBOS_BOXD_PLATFORM` | `systrap` |

A box's sandbox, network namespace and cgroup are all named `arbos-box-<user_id>.`, with the
dot: runsc takes an id that is the start of exactly one sandbox's as that sandbox, and no user
id holds a dot.

### What it leaves on the host

The daemon does not undo its host setup when it exits, and there is no call that deletes a
box. Stopping it for good, after every box is asleep, leaves these to remove by hand:

- the bridge `arbos-boxd0` (`ip link del arbos-boxd0`), and the firewall rules above;
- the tmpfs `ip netns` mounts on `/run/netns`;
- runsc's own mount at `<ARBOS_BOXD_RUNSC_ROOT>/null-netns`;
- the controllers runsc turned on in the root's `cgroup.subtree_control` (`cpuset io hugetlb
  rdma misc` beside systemd's own), and the cgroup `/sys/fs/cgroup/arbos-boxd` itself if a
  sandbox did not exit clean;
- the files under `ARBOS_BOXD_ROOT`.

Making or removing the bridge is a link change every network daemon on the host sees:
tailscaled rebinds on it and restarts `systemd-resolved`.

## HTTP

Every request carries `Authorization: Bearer <token>`; anything else is a bare 401. Errors
are `{"error": "..."}`.

| Request | Answer |
| --- | --- |
| `POST /v1/boxes/{user_id}/ensure` with `{"version": "0.6.409", "env": {...}}` | `{"created", "size_gb"}`. 409 if the version is not installed, or if it would change the bundle of a box that is running or checkpointed. |
| `POST /v1/boxes/{user_id}/wake` | `{"state": "warm", "restored", "box_url"}`. Does nothing to a running box. |
| `POST /v1/boxes/{user_id}/sleep` | `{"state": "asleep_clean" \| "asleep_ckpt", "files_bytes", "scratch_bytes", "trash_bytes", "work_bytes", "ckpt_bytes"}`. 409 `busy: <reason>` when agentd refuses, and the box keeps running. 502 when agentd does not answer. |
| `POST /v1/boxes/{user_id}/grow` | `{"size_gb"}`: 8 → 32 → 128. 409 at 128, or while the box is asleep. |
| `GET /v1/host/status` | `mem_available_gb`, `swap_used_gb`, `load5`, `disk_free_gb`, `resident_boxes`, `present_boxes`, and `boxes[]` with `user_id`, `running`, `size_gb`, `work_bytes`, `ckpt_bytes`. |

Known limit: a box with any workspace is always checkpointed, never stopped clean. Inside
gVisor agentd has no writable cgroup v2, so it cannot count a workspace's processes and
reports `active_user_pids: null`, which is not zero. Only a box with no workspace sleeps clean.

`env` is the box's environment (the relay's `boxes::box_env`). `ARBOS_MCP_TOKEN` in it is the
bearer the daemon uses for agentd's `POST /box/prepare-sleep`. `box_url` is the box's own
address on the bridge, port 7788; no port is published on the host.

## Testing

`cargo test -p arbos-boxd` runs the state machine against a fake host, anywhere. The real
path (loop devices, ext4, network namespaces, runsc checkpoint and restore) is
`scripts/ci/boxd-linux-test.sh`, which needs only Docker.

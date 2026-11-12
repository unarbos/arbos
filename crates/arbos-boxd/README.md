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

## HTTP

Every request carries `Authorization: Bearer <token>`; anything else is a bare 401. Errors
are `{"error": "..."}`.

| Request | Answer |
| --- | --- |
| `POST /v1/boxes/{user_id}/ensure` with `{"version": "0.6.409", "env": {...}}` | `{"created", "size_gb"}`. 409 if the version is not installed, or if it would change the bundle of a box that is running or checkpointed. |
| `POST /v1/boxes/{user_id}/wake` | `{"state": "warm", "restored", "box_url"}`. Does nothing to a running box. |
| `POST /v1/boxes/{user_id}/sleep` | `{"state": "asleep_clean" \| "asleep_ckpt", "files_bytes", "scratch_bytes", "trash_bytes", "work_bytes", "ckpt_bytes"}`. 409 `busy: <reason>` when agentd refuses, and the box keeps running. 502 when agentd does not answer. |
| `POST /v1/boxes/{user_id}/sleep?force=true` | The same answer, always `asleep_clean` and with a null split. agentd is not asked: a busy box is stopped too, and no checkpoint is written or kept, so every process in the box ends. `/work` is untouched. This is the user's Stop and an operator's delete of the box. |
| `POST /v1/boxes/{user_id}/grow` | `{"size_gb"}`: 8 → 32 → 128. 409 at 128, or while the box is asleep. |
| `GET /v1/host/status` | `mem_available_gb`, `swap_used_gb`, `load5`, `disk_free_gb`, `resident_boxes`, `present_boxes`, and `boxes[]` with `user_id`, `running`, `size_gb`, `work_bytes`, `ckpt_bytes`. |

`env` is the box's environment (the relay's `boxes::box_env`). `ARBOS_MCP_TOKEN` in it is the
bearer the daemon uses for agentd's `POST /box/prepare-sleep`. `box_url` is the box's own
address on the bridge, port 7788; no port is published on the host.

## Testing

`cargo test -p arbos-boxd` runs the state machine against a fake host, anywhere. The real
path (loop devices, ext4, network namespaces, runsc checkpoint and restore) is
`scripts/ci/boxd-linux-test.sh`, which needs only Docker.

//! The OCI `config.json` of a box, and of the one-shot sandbox that seeds its `/etc`.
//!
//! The box bundle says what `fleet::run_args` in the relay says to Docker today — same mounts,
//! same dropped capabilities, read-only root — so the same image runs the same way. Only the
//! limits differ: docs/box-scaling.md §4.1 raises them to 8 vCPU and 16 GiB.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::{json, Value};

pub const BOX_CPUS: i64 = 8;
pub const BOX_MEM_BYTES: i64 = 16 << 30;
/// Unchanged from the Docker fleet (`ARBOS_BOX_PIDS`).
pub const BOX_PIDS: i64 = 512;

/// Where the image's version is kept in a box's `config.json`.
pub const VERSION_ANNOTATION: &str = "life.arbos.image-version";

/// Docker's default capability set minus what the fleet drops (NET_RAW, MKNOD, AUDIT_WRITE).
const CAPS: &[&str] = &[
    "CAP_CHOWN",
    "CAP_DAC_OVERRIDE",
    "CAP_FSETID",
    "CAP_FOWNER",
    "CAP_SETGID",
    "CAP_SETUID",
    "CAP_SETFCAP",
    "CAP_SETPCAP",
    "CAP_NET_BIND_SERVICE",
    "CAP_SYS_CHROOT",
    "CAP_KILL",
];

/// What a rootfs tar does not carry: `docker image inspect --format '{{json .Config}}'`,
/// saved beside it as `image.json`. Only these fields are read.
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Image {
    #[serde(default)]
    entrypoint: Option<Vec<String>>,
    #[serde(default)]
    cmd: Option<Vec<String>>,
    #[serde(default)]
    env: Option<Vec<String>>,
    #[serde(default)]
    working_dir: String,
}

fn tmpfs(dst: &str, size: &str) -> Value {
    // Docker's `--tmpfs` defaults, which the fleet's boxes have always had.
    json!({ "destination": dst, "type": "tmpfs", "source": "tmpfs",
            "options": ["nosuid", "nodev", "noexec", format!("size={size}")] })
}

fn bind(src: &str, dst: &str, mode: &str) -> Value {
    json!({ "destination": dst, "type": "bind", "source": src, "options": ["rbind", mode] })
}

fn caps() -> Value {
    json!({ "bounding": CAPS, "effective": CAPS, "permitted": CAPS })
}

/// Where a box's files are on the host.
pub struct BoxPaths<'a> {
    pub rootfs: &'a str,
    pub mnt: &'a str,
    pub resolv: &'a str,
    pub hosts: &'a str,
    pub netns: &'a str,
}

pub fn box_config(
    id: &str,
    version: &str,
    image: &Image,
    env: &BTreeMap<String, String>,
    p: &BoxPaths,
) -> Value {
    // The image's environment, with the relay's on top.
    let mut vars: BTreeMap<&str, &str> = image
        .env
        .iter()
        .flatten()
        .filter_map(|kv| kv.split_once('='))
        .collect();
    vars.extend(env.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    let env: Vec<String> = vars.iter().map(|(k, v)| format!("{k}={v}")).collect();
    let args: Vec<&String> = image
        .entrypoint
        .iter()
        .chain(&image.cmd)
        .flatten()
        .collect();
    let cwd = if image.working_dir.is_empty() {
        "/"
    } else {
        &image.working_dir
    };
    json!({
        "ociVersion": "1.0.2",
        "annotations": { VERSION_ANNOTATION: version },
        "hostname": "arbos-box",
        // Root in the image is the Sentry's root, not the host's (the fleet's `-u 0`).
        "process": {
            "user": { "uid": 0, "gid": 0 },
            "args": args,
            "env": env,
            "cwd": cwd,
            "capabilities": caps(),
            "noNewPrivileges": true,
        },
        "root": { "path": p.rootfs, "readonly": true },
        "mounts": [
            { "destination": "/proc", "type": "proc", "source": "proc" },
            { "destination": "/dev", "type": "tmpfs", "source": "tmpfs" },
            { "destination": "/sys", "type": "sysfs", "source": "sysfs",
              "options": ["nosuid", "noexec", "nodev", "ro"] },
            tmpfs("/dev/shm", "256m"),
            tmpfs("/tmp", "512m"),
            tmpfs("/run", "16m"),
            bind(p.mnt, "/work", "rw"),
            bind(&format!("{}/.home", p.mnt), "/home/daytona", "rw"),
            // A writable copy of the image's /etc on the user's own disk: agentd's per-project
            // Unix users must survive a restart while the root stays read-only.
            bind(&format!("{}/.etc", p.mnt), "/etc", "rw"),
            bind(p.resolv, "/etc/resolv.conf", "ro"),
            bind(p.hosts, "/etc/hosts", "ro"),
        ],
        "linux": {
            "namespaces": [
                { "type": "pid" }, { "type": "ipc" }, { "type": "uts" }, { "type": "mount" },
                { "type": "network", "path": p.netns },
            ],
            "cgroupsPath": format!("/arbos-boxd/{id}"),
            "resources": {
                "memory": { "limit": BOX_MEM_BYTES, "swap": BOX_MEM_BYTES, "reservation": 512 << 20 },
                "cpu": { "shares": 512, "quota": BOX_CPUS * 100_000, "period": 100_000 },
                "pids": { "limit": BOX_PIDS },
            },
        },
    })
}

/// Account files the box owns: agentd's per-project users and groups. They are why `/etc` is
/// persisted at all; everything else under `/etc` belongs to the image and follows it.
const ETC_BOX_OWNED: &[&str] = &[
    "passwd",
    "passwd-",
    "group",
    "group-",
    "shadow",
    "shadow-",
    "gshadow",
    "gshadow-",
    "subuid",
    "subuid-",
    "subgid",
    "subgid-",
    ".pwd.lock",
];

/// Image-owned trees under `/etc` that are replaced exactly, so a file the image dropped goes.
const ETC_IMAGE_TREES: &[&str] = &["arbos"];

/// The file in a box's `/etc` that names the image version it was last seeded from.
pub const ETC_SEED_MARKER: &str = ".arbos-seed-image";

/// The shell that brings a persisted `/etc` (mounted at `/mnt`) in line with the image it runs
/// in. The relay's `fleet::etc_refresh_script`, kept the same so a box moved from Docker is
/// refreshed the way it always was. `version` is allowlisted by the caller.
fn etc_refresh_script(first_boot: bool, version: &str) -> String {
    let mark = format!("printf '%s\\n' '{version}' > /mnt/{ETC_SEED_MARKER}");
    if first_boot {
        return format!("set -e; cp -a /etc/. /mnt/; {mark}");
    }
    let each = |of: &[&str], f: fn(&str) -> String| -> String {
        of.iter().map(|x| f(x)).collect::<Vec<_>>().join("; ")
    };
    format!(
        "set -e; mkdir -p /tmp/keep; {keep}; {trees}; cp -a /etc/. /mnt/; {restore}; {mark}",
        keep = each(ETC_BOX_OWNED, |f| format!(
            "[ -e /mnt/{f} ] && cp -a /mnt/{f} /tmp/keep/{f} || true"
        )),
        trees = each(ETC_IMAGE_TREES, |t| format!(
            "rm -rf /mnt/{t}; [ -e /etc/{t} ] && cp -a /etc/{t} /mnt/{t} || true"
        )),
        restore = each(ETC_BOX_OWNED, |f| format!(
            "[ -e /tmp/keep/{f} ] && cp -a /tmp/keep/{f} /mnt/{f} || true"
        )),
    )
}

/// The bundle of the sandbox that seeds `etc` from the image. The copy runs inside gVisor, not
/// as host root, because `etc` is the user's to write: a link planted there resolves inside
/// the sandbox and reaches nothing of the host's.
pub fn seed_config(rootfs: &str, etc: &str, first_boot: bool, version: &str) -> Value {
    json!({
        "ociVersion": "1.0.2",
        "process": {
            "user": { "uid": 0, "gid": 0 },
            "args": ["sh", "-c", etc_refresh_script(first_boot, version)],
            "env": ["PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"],
            "cwd": "/",
            "capabilities": caps(),
            "noNewPrivileges": true,
        },
        "root": { "path": rootfs, "readonly": true },
        // Only on directories every image has: runsc makes a missing mount point, and it would
        // make it in the rootfs all boxes share.
        "mounts": [
            { "destination": "/proc", "type": "proc", "source": "proc" },
            { "destination": "/dev", "type": "tmpfs", "source": "tmpfs" },
            tmpfs("/tmp", "64m"),
            bind(etc, "/mnt", "rw"),
        ],
        "linux": {
            "namespaces": [
                { "type": "pid" }, { "type": "ipc" }, { "type": "uts" }, { "type": "mount" },
                { "type": "network" },
            ],
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image() -> Image {
        serde_json::from_value(json!({
            "Entrypoint": ["/usr/local/bin/arbos-agentd", "boot"],
            "Cmd": null,
            "Env": ["PATH=/usr/bin", "ARBOS_PORT=7788", "HOME=/home/daytona"],
            "WorkingDir": "/work",
            "User": "daytona",
        }))
        .unwrap()
    }

    /// The bundle is the Docker fleet's box: the image's entrypoint and environment with the
    /// relay's on top, the user's disk at the three places it has always been, a read-only
    /// root, and none of the capabilities the fleet drops.
    #[test]
    fn the_bundle_runs_the_box_the_docker_fleet_runs() {
        let env = BTreeMap::from([
            ("ARBOS_MCP_TOKEN".to_owned(), "t".to_owned()),
            ("HOME".to_owned(), "/elsewhere".to_owned()),
        ]);
        let p = BoxPaths {
            rootfs: "/i/rootfs",
            mnt: "/b/mnt",
            resolv: "/r",
            hosts: "/h",
            netns: "/run/netns/n",
        };
        let c = box_config("arbos-box-u", "0.6.409", &image(), &env, &p);
        assert_eq!(
            c["process"]["args"],
            json!(["/usr/local/bin/arbos-agentd", "boot"])
        );
        assert_eq!(
            c["process"]["env"],
            json!([
                "ARBOS_MCP_TOKEN=t",
                "ARBOS_PORT=7788",
                "HOME=/elsewhere",
                "PATH=/usr/bin"
            ])
        );
        assert_eq!(c["process"]["cwd"], "/work");
        assert_eq!(c["process"]["user"]["uid"], 0);
        assert_eq!(c["root"], json!({ "path": "/i/rootfs", "readonly": true }));
        let mounts = c["mounts"].as_array().unwrap();
        let src = |dst: &str| {
            let m = mounts.iter().find(|m| m["destination"] == dst).unwrap();
            m["source"].as_str().unwrap().to_owned()
        };
        assert_eq!(src("/work"), "/b/mnt");
        assert_eq!(src("/home/daytona"), "/b/mnt/.home");
        assert_eq!(src("/etc"), "/b/mnt/.etc");
        assert_eq!(src("/etc/resolv.conf"), "/r");
        let caps = c["process"]["capabilities"]["bounding"].to_string();
        for dropped in ["NET_RAW", "NET_ADMIN", "MKNOD", "AUDIT_WRITE", "SYS_ADMIN"] {
            assert!(!caps.contains(dropped), "{dropped} is granted");
        }
        assert_eq!(c["linux"]["resources"]["memory"]["limit"], 16i64 << 30);
        assert_eq!(c["linux"]["resources"]["cpu"]["quota"], 800_000);
        assert_eq!(c["annotations"][VERSION_ANNOTATION], "0.6.409");
    }

    /// A refresh keeps the box's own account files and replaces the image's trees; a first
    /// boot copies everything. Both leave the marker.
    #[test]
    fn the_etc_seed_keeps_account_files_on_a_refresh() {
        let first = etc_refresh_script(true, "1.2.3");
        assert!(first.starts_with("set -e; cp -a /etc/. /mnt/;"));
        let again = etc_refresh_script(false, "1.2.3");
        assert!(again.contains("cp -a /mnt/passwd /tmp/keep/passwd"));
        assert!(again.contains("cp -a /tmp/keep/passwd /mnt/passwd"));
        assert!(again.contains("rm -rf /mnt/arbos;"));
        for s in [first, again] {
            assert!(s.ends_with("printf '%s\\n' '1.2.3' > /mnt/.arbos-seed-image"));
        }
    }
}

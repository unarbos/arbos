"""Run scripts on the test agent's sandbox. Two backends: a D89 host sandbox (ssh to the relay
host, `docker exec` into `arbos-box-<user>`) and a Daytona box (toolbox API). The eval only ever
touches the dedicated test user's sandbox."""

import base64
import hashlib
import json
import os
import re
import shlex
import subprocess

from . import relay as rl

SSH_HOST = os.environ.get("LT_HOST_SSH", "const@204.12.171.6")
SSH_KEY = os.environ.get("LT_HOST_SSH_KEY", os.path.expanduser("~/.ssh/arbos_cloud"))
DAYTONA_KEY = os.environ.get("DAYTONA_API_KEY", "")


class Box:
    def __init__(self):
        m = rl.machine()
        self.provider = m.get("provider") or "gvisor"
        self.sandbox_id = m.get("sandbox_id")
        self.image = ((m.get("image") or {}).get("current")) or ((m.get("image") or {}).get("default"))
        self.user = rl.user_id()
        self.container = f"arbos-box-{self.user.lower()}"

    # ---- raw exec ----------------------------------------------------------------------------

    def _exec_root(self, script, timeout):
        b64 = base64.b64encode(script.encode()).decode()
        if self.provider == "daytona":
            st, v = rl.http(
                "POST",
                f"https://proxy.app.daytona.io/toolbox/{self.sandbox_id}/process/execute",
                token=DAYTONA_KEY,
                body={"command": f"echo {b64} | base64 -d | sudo -n bash", "timeout": timeout},
                timeout=timeout + 30,
            )
            if st != 200:
                return 255, f"toolbox exec {st}: {v}"
            return v.get("exitCode"), v.get("result", "")
        # the script travels on stdin (base64, one line) so its size never hits the argv limit
        cmd = f"docker exec -i {shlex.quote(self.container)} bash -c 'read -r B; echo \"$B\" | base64 -d | bash'"
        p = subprocess.run(
            ["ssh", "-i", SSH_KEY, "-o", "BatchMode=yes", "-o", "ConnectTimeout=20", "-o", "ServerAliveInterval=15", SSH_HOST, cmd],
            input=b64 + "\n",
            capture_output=True,
            text=True,
            timeout=timeout + 60,
        )
        return p.returncode, p.stdout + (("\n" + p.stderr) if p.stderr.strip() else "")

    def root(self, script, timeout=120):
        try:
            return self._exec_root(script, timeout)
        except subprocess.TimeoutExpired:
            return 124, "box exec timed out"

    def ws_user(self, project):
        code, out = self.root(f"stat -c %U /work/{project}")
        return out.strip().splitlines()[0] if out.strip() else "root"

    def run_as_ws(self, project, script, env=None, timeout=120):
        """Run a bash script as the workspace user in the project directory."""
        pre = f"cd /work/{project} && "
        for k, v in (env or {}).items():
            pre += f"export {k}={shlex.quote(str(v))}; "
        user = self.ws_user(project)
        b64 = base64.b64encode((pre + script).encode()).decode()
        return self.root(f"echo {b64} | base64 -d | sudo -n -u {shlex.quote(user)} bash", timeout)

    def put_files(self, project, base_dir, files, timeout=180):
        """Write {relative path: bytes} under /work/<project>/<base_dir> as the workspace user."""
        parts = []
        for rel, data in files.items():
            b64 = base64.b64encode(data).decode()
            parts.append(f"mkdir -p \"$(dirname {shlex.quote(rel)})\" && echo {b64} | base64 -d > {shlex.quote(rel)}")
        script = f"mkdir -p {shlex.quote(base_dir)} && cd {shlex.quote(base_dir)} && " + " && ".join(parts) + " && echo PUT_OK"
        code, out = self.run_as_ws(project, script, timeout=timeout)
        return "PUT_OK" in out, out[-300:]

    def read_file(self, project, path, limit=6000):
        code, out = self.root(f"head -c {limit} /work/{project}/{shlex.quote(path)} 2>/dev/null")
        return out

    # ---- evidence ----------------------------------------------------------------------------

    def session_entries(self, project, chat_id):
        code, out = self.root(f"cat /work/{project}/.arbos/pi/sessions/*_{chat_id}.jsonl 2>/dev/null")
        entries = []
        for line in out.splitlines():
            line = line.strip()
            if not line.startswith("{"):
                continue
            try:
                entries.append(json.loads(line))
            except json.JSONDecodeError:
                pass
        return entries

    def audit_since(self, project, ts_ms):
        code, out = self.root(f"cat /work/{project}/.arbos/audit.jsonl 2>/dev/null")
        rows = []
        for line in out.splitlines():
            try:
                v = json.loads(line)
            except json.JSONDecodeError:
                continue
            if v.get("ts", 0) >= ts_ms:
                rows.append(v)
        return rows

    def agentd_sha(self):
        code, out = self.root("cat /opt/arbos/agentd.sha 2>/dev/null || arbos-agentd --version 2>/dev/null || cat /etc/arbos-image 2>/dev/null; true")
        return out.strip()[:80]

    def relay_build(self):
        """What relay binary serves this run: the deployed file's mtime and the commit sha embedded in it
        (the relay has no /version; a P0 rollback mid-run must be visible in the record)."""
        if self.provider == "daytona":
            return {}
        # The embedded-string sha was the same across three relay deploys in run 12 (it is a
        # dependency's hash, not the build's), so the deploy log name — `relay-<sha>.log`,
        # written by the deploy script next to the binary — is the commit when present.
        cmd = ("stat -c %y /usr/local/bin/arbos-relay; "
               "ls -t /var/lib/arbos-deploy/runs/relay-*.log 2>/dev/null | head -1 | grep -oE '[0-9a-f]{40}' || "
               "strings /usr/local/bin/arbos-relay | grep -oE '[0-9a-f]{40}' | sort | uniq -c | sort -rn | head -1")
        p = subprocess.run(["ssh", "-i", SSH_KEY, "-o", "BatchMode=yes", "-o", "ConnectTimeout=20", SSH_HOST, cmd], capture_output=True, text=True, timeout=90)
        lines = p.stdout.strip().splitlines()
        sha = lines[1].split()[-1] if len(lines) > 1 and lines[1].split() else None
        return {"binary_mtime": lines[0][:19] if lines else None, "sha": sha}

    def refresh_etc_arbos(self):
        """Re-copy `/etc/arbos` (pi-seed, claude-seed, prime-seed, AGENTS.md) from the box's current
        image into its persisted `/etc` (fleet.rs `seed_etc` copies the image's /etc only on the
        box's first boot, so a host-sandbox box keeps the seed of the image that created it). Test
        box only; the box must be restarted afterwards so agentd re-seeds the project pi dirs."""
        if self.provider == "daytona":
            return 1, "refresh is for host sandboxes only"
        etc = f"/var/lib/arbos-relay/boxes/{self.user}/mnt/.etc"
        image = subprocess.run(
            ["ssh", "-i", SSH_KEY, "-o", "BatchMode=yes", "-o", "ConnectTimeout=20", SSH_HOST,
             f"docker inspect {shlex.quote(self.container)} --format '{{{{.Config.Image}}}}'"],
            capture_output=True, text=True, timeout=60,
        ).stdout.strip()
        if not image:
            return 1, "could not read the box's image"
        cmd = (f"docker run --rm -u 0 --network none --mount type=bind,src={shlex.quote(etc)},dst=/seed "
               f"--entrypoint sh {shlex.quote(image)} -c 'rm -rf /seed/arbos && cp -a /etc/arbos /seed/arbos && cat /seed/arbos/pi-seed/extensions/arbos-pi-ext/.source-sha'")
        p = subprocess.run(["ssh", "-i", SSH_KEY, "-o", "BatchMode=yes", "-o", "ConnectTimeout=20", SSH_HOST, cmd], capture_output=True, text=True, timeout=180)
        return p.returncode, (p.stdout + p.stderr).strip()

    def pi_seed(self, repo_root):
        """The arbos-pi-ext seed the box runs vs the commit its image was built from (#1702: images
        0.1.121–0.1.124 shipped a three-day-old seed next to a fresh agentd). Returns a dict with
        `ok`: True when `.source-sha` equals the image tag's commit, or — for images that predate the
        marker — when index.ts in the box is byte-identical to the file at that commit. A mismatch
        means the run would measure the wrong pi extension and must not count as a result."""
        version = (self.image or "").replace("arbos-agent-", "")
        rec = {"image": self.image, "tag": f"image-v{version}" if version else None, "tag_commit": None, "source_sha": None,
               "index_sha256_box": None, "index_sha256_repo": None, "ok": None, "reason": ""}
        if not version:
            rec["reason"] = "box image unknown"
            return rec
        try:
            # the tag may be newer than the clone: fetch it, then resolve with --verify so a missing tag is
            # "cannot verify" (#1731: plain rev-parse echoed the argument back and read as a mismatch)
            subprocess.run(["git", "-C", str(repo_root), "fetch", "-q", "origin", f"refs/tags/{rec['tag']}:refs/tags/{rec['tag']}"], capture_output=True, text=True, timeout=120)
            rp = subprocess.run(["git", "-C", str(repo_root), "rev-parse", "--verify", "-q", f"{rec['tag']}^{{commit}}"], capture_output=True, text=True, timeout=30)
            commit = rp.stdout.strip()
            rec["tag_commit"] = commit if rp.returncode == 0 and re.fullmatch(r"[0-9a-f]{40}", commit) else None
            if rec["tag_commit"]:
                shown = subprocess.run(["git", "-C", str(repo_root), "show", f"{rec['tag_commit']}:harness/arbos-pi-ext/index.ts"], capture_output=True, timeout=30)
                rec["index_sha256_repo"] = hashlib.sha256(shown.stdout).hexdigest() if shown.returncode == 0 else None
        except Exception as e:  # noqa: BLE001
            rec["reason"] = f"repo lookup: {e}"
        code, out = self.root("D=/etc/arbos/pi-seed/extensions/arbos-pi-ext; cat $D/.source-sha 2>/dev/null | tr -d '[:space:]'; echo; sha256sum $D/index.ts 2>/dev/null | cut -d' ' -f1; true")
        lines = (out or "").split("\n")
        rec["source_sha"] = (lines[0].strip() or None) if lines else None
        rec["index_sha256_box"] = (lines[1].strip() or None) if len(lines) > 1 else None
        if not rec["tag_commit"]:
            # ok stays None: cannot verify (blocked with this reason), which is not a mismatch
            rec["reason"] = rec["reason"] or f"tag {rec['tag']} not found on origin or locally — cannot verify the seed"
            return rec
        if rec["source_sha"]:
            rec["ok"] = rec["source_sha"] == rec["tag_commit"]
            rec["reason"] = "seed .source-sha matches the image commit" if rec["ok"] else f"seed from {rec['source_sha'][:12]} but image built from {rec['tag_commit'][:12]}"
        elif rec["index_sha256_box"] and rec["index_sha256_repo"]:
            rec["ok"] = rec["index_sha256_box"] == rec["index_sha256_repo"]
            rec["reason"] = "no .source-sha marker (pre-#1703 image); index.ts identical to the image commit's" if rec["ok"] else "no .source-sha marker and index.ts differs from the image commit's — stale seed"
        else:
            rec["reason"] = "could not read the seed in the box"
        return rec

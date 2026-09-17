"""Arbos as a verifiers v1 harness: the kernel plays the agent seat.

The program is `arbos-swe-run` (bundled here), which runs `arbos-kernel run`
headless inside the task's container, then leaves the patch, the exported
rollout bundle, and a `result.json` under `/tmp/vf-arbos/out`, which the
harness copies to the host. Model calls go to the interception endpoint as a
custom OpenAI-compatible provider, so the trace verifiers records is the sample.

    uv run eval swebench-verified --env.agent.harness.id arbos-harness \
        --env.agent.runtime.type docker --env.agent.runtime.block '["*"]' -m <model> \
        --client.base-url https://openrouter.ai/api/v1 --client.api-key-var OPENROUTER_API_KEY

`--env.agent.runtime.block '["*"]'` leaves the container only the interception
route, and the harness refuses to run without it: with the docker runtime's
default host network the agent `pip download`s the release that already carries
the fix (133 of 948 rollouts across the SWE-bench loop's first eleven cycles,
114 of them graded solved), so an open-network rollout is not a measurement.
`--env.agent.harness.allow-open-egress true` overrides the refusal for debugging;
the `arbos_egress_open` metric then marks those rollouts.
"""

from __future__ import annotations

import base64
import io
import json
import logging
import os
import re
import shlex
import shutil
import subprocess
import tarfile
from pathlib import Path

from pydantic import Field
from verifiers.v1 import metric
from verifiers.v1.clients import ModelContext
from verifiers.v1.configs.harness import HarnessConfig
from verifiers.v1.harness import Harness
from verifiers.v1.runtimes import ProgramResult, Runtime
from verifiers.v1.task import TaskData
from verifiers.v1.trace import Trace

logger = logging.getLogger(__name__)

PROGRAM = Path(__file__).resolve().parent / "arbos-swe-run"
BIN_DIR = "/tmp/vf-arbos/bin"
KERNEL_BIN = f"{BIN_DIR}/arbos-kernel"
PROGRAM_BIN = f"{BIN_DIR}/arbos-swe-run"
# Not under /logs/artifacts: Harbor ships that whole tree to the grading box
# with a 32 MB cap, and a long rollout's trace/ alone passes it (django-15629
# errored in finalize). The harness collects this dir itself.
OUT_DIR = "/tmp/vf-arbos/out"
DEFAULT_IMAGE = "arbos-harness"
CACHE = Path(os.environ.get("XDG_CACHE_HOME", Path.home() / ".cache")) / "arbos-harness"


class ArbosHarnessConfig(HarnessConfig):
    kernel: str = ""
    """Host path to a static `arbos-kernel` (musl). Empty = `$ARBOS_KERNEL`, else
    the binary inside the `image` Docker image."""
    image: str = DEFAULT_IMAGE
    """Docker image the kernel is copied out of when `kernel` is empty
    (`docker build -f harness/Dockerfile -t arbos-harness .`)."""
    timeout: float = Field(2400.0, gt=0)
    """Seconds the agent's turn may take before the kernel stops it."""
    mode: str = Field("auto", pattern=r"^(auto|ask|plan)$")
    """Permission mode of the root agent; headless runs want `auto`."""
    trace: bool = True
    """Record every provider call in the rollout bundle (exact replay)."""
    window_tokens: int = Field(200_000, ge=0)
    """Context the kernel plans against; 0 asks the endpoint's model list."""
    allowlist: list[str] = Field(default_factory=list)
    """Tools the agent may use. Empty = the program's headless default."""
    instructions: str = ""
    """Standing instructions shown in every prompt. Empty = the program's default."""
    repro_required: int = Field(1, ge=0)
    """Failing reproductions the first edit needs (`ARBOS_REPRO_REQUIRED`): 0 = no gate,
    1 = one, 2 = the reporter's example plus a second input the agent derives."""
    mechanism_required: bool = True
    """Refuse the first edit without a `mechanism` line (`ARBOS_MECHANISM_REQUIRED`)."""
    max_turn_cost_usd: float = Field(8.0, ge=0)
    """Dollars one rollout's turn may spend on model calls before the kernel ends it
    (`ARBOS_MAX_TURN_COST`); 0 = no cap. One SWE-bench rollout ran to $14 before this;
    at $4 the cap cut three hard rollouts that had solved at $6 before (cycle 9), so $8."""
    changes_before_done: bool = False
    """Nudge a final reply after edits to run `changes` first (`ARBOS_CHANGES_BEFORE_DONE`)."""
    allow_open_egress: bool = False
    """Run even when the runtime's egress is unrestricted. Off: setup refuses, the
    rollout is an error and gets no score. On: it runs, and `arbos_egress_open` is 1.0."""
    artifacts: str = "outputs/arbos"
    """Host folder that receives each rollout's `/logs/artifacts/arbos` (patch,
    rollout bundle, kernel log, result.json) under `<task>--<trace id>/`. Empty = keep
    them in the container only."""


class ArbosHarness(Harness[ArbosHarnessConfig]):
    APPENDS_SYSTEM_PROMPT = False
    SUPPORTS_MCP = False

    def kernel_path(self) -> Path:
        configured = self.config.kernel or os.environ.get("ARBOS_KERNEL", "")
        if configured:
            path = Path(configured).expanduser()
            if not path.is_file():
                raise FileNotFoundError(f"arbos-kernel not found at {path}")
            return path
        return kernel_from_image(self.config.image)

    async def setup(self, runtime: Runtime) -> None:
        if not runtime.network_restricted and not self.config.allow_open_egress:
            raise RuntimeError(
                "arbos: the runtime's egress is open, so the agent could fetch the upstream "
                "fix; this rollout is not a measurement and is refused. Pass "
                "--env.agent.runtime.block '[\"*\"]' (or allow-open-egress true to run anyway)."
            )
        kernel = self.kernel_path()
        logger.info("arbos: installing %s into the runtime", kernel)
        await runtime.write(KERNEL_BIN, kernel.read_bytes())
        await runtime.write(PROGRAM_BIN, PROGRAM.read_bytes())
        result = await runtime.run(
            ["sh", "-c", f"chmod +x {KERNEL_BIN} {PROGRAM_BIN} && mkdir -p {OUT_DIR}"],
            {},
        )
        if result.exit_code != 0:
            raise RuntimeError(f"arbos install failed: {result.stderr.strip()[-500:]}")

    async def launch(
        self,
        ctx: ModelContext,
        trace: Trace,
        runtime: Runtime,
        endpoint: str,
        secret: str,
        mcp_urls: dict[str, str],
        data: TaskData,
    ) -> ProgramResult:
        _, prompt = self.resolve_text_prompt(data)
        if prompt is None:
            raise ValueError("arbos needs a text prompt; the task has none")
        allow = list(self.config.allowlist)
        for tool in self.config.disabled_tools or []:
            if tool in allow:
                allow.remove(tool)
        env = {
            **self.config.resolved_env,
            "ARBOS_API_KEY": secret,
            "ARBOS_API_BASE": endpoint.rstrip("/"),
            "ARBOS_PROVIDER": "custom",
            "ARBOS_MODEL": ctx.model,
            "ARBOS_MODE": self.config.mode,
            "ARBOS_TIMEOUT": str(int(self.config.timeout)),
            "ARBOS_TRACE": "1" if self.config.trace else "0",
            "ARBOS_WINDOW_TOKENS": str(self.config.window_tokens),
            "ARBOS_REPRO_REQUIRED": str(self.config.repro_required),
            "ARBOS_MECHANISM_REQUIRED": "1" if self.config.mechanism_required else "0",
            "ARBOS_MAX_TURN_COST": str(self.config.max_turn_cost_usd),
            "ARBOS_CHANGES_BEFORE_DONE": "1" if self.config.changes_before_done else "0",
            "ARBOS_OUT": OUT_DIR,
            "ARBOS_KERNEL_BIN": KERNEL_BIN,
            "XDG_CONFIG_HOME": f"/tmp/vf-arbos/{trace.id}/config",
        }
        if data.workdir:
            env["ARBOS_PLACE"] = data.workdir
        if allow:
            env["ARBOS_ALLOWLIST"] = ", ".join(allow)
        elif self.config.disabled_tools:
            raise ValueError(
                "arbos: disabled_tools needs an explicit allowlist to remove from"
            )
        if self.config.instructions:
            env["ARBOS_INSTRUCTIONS"] = self.config.instructions
        return await runtime.run_program([PROGRAM_BIN, prompt], env)

    async def result(self, runtime: Runtime) -> dict:
        try:
            raw = await runtime.read(f"{OUT_DIR}/result.json")
        except Exception:
            return {}
        try:
            return json.loads(raw.decode(errors="replace"))
        except json.JSONDecodeError:
            return {}

    @metric
    async def arbos(
        self, task: TaskData, trace: Trace, runtime: Runtime
    ) -> dict[str, float]:
        """What the kernel reported: its exit code (0 ok, 2 failed turn,
        3 waited on a question, 4 timed out), the patch size, tool calls, time.
        Also brings the rollout's artifacts to the host when `artifacts` is set."""
        r = await self.result(runtime)
        code = int(r.get("kernel_exit", -1))
        collected = 0
        if self.config.artifacts:
            collected = await self.collect(task, trace, runtime)
        return {
            "arbos_exit": float(code),
            "arbos_completed": 1.0 if code == 0 else 0.0,
            "arbos_timed_out": 1.0 if code == 4 else 0.0,
            "arbos_patch_bytes": float(r.get("patch_bytes", 0)),
            "arbos_tool_calls": float(r.get("tool_calls", 0)),
            "arbos_wall_s": float(r.get("wall_s", 0)),
            "arbos_cost_capped": float(r.get("cost_capped", 0)),
            "arbos_artifact_bytes": float(collected),
            "arbos_egress_open": 0.0 if runtime.network_restricted else 1.0,
        }

    async def collect(self, task: TaskData, trace: Trace, runtime: Runtime) -> int:
        """Copy `OUT_DIR` out of the runtime as a tarball (base64 over the run
        channel, which carries text) into `artifacts/<task>--<trace id>/`."""
        result = await runtime.run(
            ["sh", "-c", f"cd {OUT_DIR} 2>/dev/null && tar -czf - . | base64 -w0"],
            {},
        )
        if result.exit_code != 0 or not result.stdout.strip():
            logger.warning("arbos: no artifacts to collect: %s", result.stderr[-300:])
            return 0
        data = base64.b64decode(result.stdout.strip())
        name = safe_name(task.name or f"task-{task.idx}")
        dest = Path(self.config.artifacts) / f"{name}--{trace.id[:8]}"
        dest.mkdir(parents=True, exist_ok=True)
        with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as tar:
            tar.extractall(dest, filter="data")
        (dest / "task.json").write_text(
            json.dumps(
                {
                    "name": task.name,
                    "image": task.image,
                    "workdir": task.workdir,
                    "trace_id": trace.id,
                },
                indent=1,
            )
        )
        return len(data)

    async def cleanup(self, trace: Trace, runtime: Runtime) -> None:
        await runtime.run(["rm", "-rf", f"/tmp/vf-arbos/{trace.id}", OUT_DIR], {})


def safe_name(name: str) -> str:
    return re.sub(r"[^A-Za-z0-9._-]+", "_", name).strip("_") or "task"


def kernel_from_image(image: str) -> Path:
    """Copy `/usr/local/bin/arbos-kernel` out of `image`, once per image id."""
    if shutil.which("docker") is None:
        raise RuntimeError(
            "arbos: no kernel path given and docker is not installed; set "
            "--env.agent.harness.kernel or $ARBOS_KERNEL to a static arbos-kernel"
        )
    inspect = subprocess.run(
        ["docker", "image", "inspect", "--format", "{{.Id}}", image],
        capture_output=True,
        text=True,
    )
    if inspect.returncode != 0:
        raise RuntimeError(
            f"arbos: image {image!r} not found; build it with "
            f"`docker build -f harness/Dockerfile -t {shlex.quote(image)} .` "
            "or set --env.agent.harness.kernel"
        )
    image_id = inspect.stdout.strip().removeprefix("sha256:")[:16]
    dest = CACHE / image_id / "arbos-kernel"
    if dest.is_file():
        return dest
    dest.parent.mkdir(parents=True, exist_ok=True)
    create = subprocess.run(
        ["docker", "create", image], capture_output=True, text=True, check=True
    )
    container = create.stdout.strip()
    try:
        tmp = dest.with_suffix(".tmp")
        subprocess.run(
            ["docker", "cp", f"{container}:/usr/local/bin/arbos-kernel", str(tmp)],
            check=True,
            capture_output=True,
        )
        tmp.replace(dest)
    finally:
        subprocess.run(["docker", "rm", "-f", container], capture_output=True)
    return dest


__all__ = ["ArbosHarness", "ArbosHarnessConfig"]

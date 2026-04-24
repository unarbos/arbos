"""``arbos`` command-line entry point.

Subcommands:

* ``arbos install``  - per-machine bootstrap, driven by env vars set by
  ``./run.sh`` (or by interactive prompts for the four base secrets if env
  is empty).
* ``arbos status``   - print persisted install state and last error.
* ``arbos reset --hard`` - wipe ``~/.arbos/`` (typed confirmation).

All commands operate against the fixed location ``~/.arbos/`` (overridable
via the ``ARBOS_HOME`` env var, intended for tests). The cwd of invocation
no longer matters -- there is exactly one install per host.
"""

from __future__ import annotations

import asyncio
import logging
import os
import shutil
import socket

import typer
from rich.console import Console
from rich.logging import RichHandler
from rich.table import Table

from .auth import confirm_authorized
from .bot_admin import add_and_promote
from .botfather import acquire_bot, disable_privacy
from .config import InstallInputs
from .errors import InstallError
from .persist import (
    append_log,
    build_stored_config,
    write_doppler_writeback,
    write_final_config,
    write_secrets,
)
from .state import InstallState, StateStore, is_at_least
from .supergroup import (
    adopt_supergroup,
    create_supergroup,
    ensure_forum,
    supergroup_exists,
)
from .arbos_client import open_session
from .topics import (
    CreatedTopic,
    create_machine_topic,
    seed_welcome,
    topic_exists,
)
from .workspace import InstallPaths, normalize_project_name

app = typer.Typer(
    add_completion=False,
    help="Bootstrap a Telegram-driven local agent topology.",
)
console = Console()


def _setup_logging(verbose: bool) -> None:
    # Root stays at WARNING so noisy third-party loggers (notably aiotdlib's
    # per-client loggers, which are named "Client:<id>" / "Client_<id>" at
    # the top level rather than as children of "aiotdlib") get filtered out.
    # We then lift only our own package back up to the requested level.
    arbos_level = logging.DEBUG if verbose else logging.INFO
    logging.basicConfig(
        level=logging.WARNING,
        format="%(message)s",
        datefmt="[%X]",
        handlers=[RichHandler(console=console, rich_tracebacks=True, show_path=False)],
    )
    logging.getLogger("arbos").setLevel(arbos_level)
    for noisy in ("aiotdlib", "aiotdlib.tdjson", "aiotdlib.client", "asyncio"):
        logging.getLogger(noisy).setLevel(logging.WARNING)
    if not verbose:
        # Silence libtdjson's C++ stderr writer (the "[ 3][t 0]...Created
        # managed client 1" line and its kin) before the first Client is
        # constructed. Idempotent + no-op if the lib can't be loaded.
        try:
            from .arbos_client import silence_tdlib
            silence_tdlib()
        except Exception:
            pass


# Doppler-backed env vars (run.sh injects them via `doppler run`).
ENV_API_ID = "ARBOS_TELEGRAM_API_ID"
ENV_API_HASH = "ARBOS_TELEGRAM_API_HASH"
ENV_PHONE = "ARBOS_TELEGRAM_PHONE"
ENV_NAME = "ARBOS_TELEGRAM_NAME"
ENV_TWOFA = "ARBOS_TELEGRAM_2FA"

# Set directly by run.sh per machine.
ENV_MACHINE = "ARBOS_MACHINE"
ENV_BOT_TOKEN = "ARBOS_TELEGRAM_BOT_TOKEN"
ENV_CHAT_ID = "ARBOS_TELEGRAM_CHAT_ID"
ENV_SUPERGROUP_ID = "ARBOS_TELEGRAM_SUPERGROUP_ID"
ENV_TOKEN_KEY = "ARBOS_TELE_TOKEN_KEY"  # e.g. "ARBOS_FOO_TELE_TOKEN"


def _env(name: str, default: "str | None" = None) -> "str | None":
    val = os.environ.get(name)
    if val is None or val.strip() == "":
        return default
    return val.strip()


def _resolve_machine_name() -> str:
    """Use ARBOS_MACHINE if set (run.sh path), else fall back to hostname."""
    raw = _env(ENV_MACHINE) or socket.gethostname() or "local"
    return normalize_project_name(raw)


def _build_inputs() -> InstallInputs:
    """Build :class:`InstallInputs` from env, prompting for any missing base
    secret. Machine-derived fields (bot username, etc.) come from the env."""
    api_id = _env(ENV_API_ID) or typer.prompt(
        f"Telegram API id ({ENV_API_ID})", type=int
    )
    api_hash = _env(ENV_API_HASH) or typer.prompt(
        f"Telegram API hash ({ENV_API_HASH})", hide_input=True, show_default=False
    )
    phone = _env(ENV_PHONE) or typer.prompt(
        f"Phone, international (e.g. +12025550123) ({ENV_PHONE})"
    )
    name = _env(ENV_NAME) or typer.prompt(
        f"Supergroup title ({ENV_NAME})",
        default="arbos",
    )
    twofa = _env(ENV_TWOFA)

    machine = _resolve_machine_name()

    chat_id_raw = _env(ENV_CHAT_ID)
    sg_id_raw = _env(ENV_SUPERGROUP_ID)

    return InstallInputs(
        telegram_api_id=int(api_id),
        telegram_api_hash=api_hash,
        phone=phone,
        twofa_password=twofa,
        supergroup_title=name,
        machine_name=machine,
        bot_token=_env(ENV_BOT_TOKEN),
        supergroup_chat_id=int(chat_id_raw) if chat_id_raw else None,
        supergroup_id=int(sg_id_raw) if sg_id_raw else None,
    )


async def _run_install(paths: InstallPaths, inputs: InstallInputs) -> None:
    paths.bootstrap()
    append_log(paths, f"install start: arbos={paths.arbos} machine={inputs.machine_name}")

    store = StateStore(paths.state_path)
    snapshot = store.load()
    if snapshot.state == InstallState.INIT:
        store.set(InstallState.INIT)
    console.log(f"resuming from state: [bold]{snapshot.state.value}[/bold]")

    # Values to push back to Doppler at the end of a successful run.
    writeback: dict[str, str] = {}

    # We always open the tdlib session, even on the COMPLETE state. The
    # supergroup might have been deleted on Telegram between runs, in
    # which case we need to recreate it. The macOS libtdjson/openssl
    # teardown race is already neutralised by the os._exit(0) at the end
    # of install().
    async with open_session(inputs, paths) as client:
        # Phase 2 - arbos auth
        if not is_at_least(snapshot.state, InstallState.ARBOS_AUTH_COMPLETE):
            store.set(InstallState.ARBOS_AUTH_PENDING)
            user = await confirm_authorized(client)
            store.set(
                InstallState.ARBOS_AUTH_COMPLETE,
                user_id=user.user_id,
                first_name=user.first_name,
            )
            append_log(paths, f"arbos auth ok: user_id={user.user_id}")
            snapshot = store.load()

        # Phase 3 - Bot acquire (existing token | /token | /newbot)
        if not is_at_least(snapshot.state, InstallState.BOT_CREATED):
            store.set(InstallState.BOTFATHER_PENDING)
            bot = await acquire_bot(
                client,
                desired_username=inputs.bot_username,
                display_name=inputs.bot_display_name,
                existing_token=inputs.bot_token,
            )
            store.set(
                InstallState.BOT_CREATED,
                bot_username=bot.username,
                bot_token=bot.token,
                bot_user_id=bot.user_id,
            )
            token_key = _env(ENV_TOKEN_KEY) or f"ARBOS_{inputs.machine_name.upper()}_TELE_TOKEN"
            writeback[token_key] = bot.token
            append_log(paths, f"bot acquired: @{bot.username} user_id={bot.user_id}")
            snapshot = store.load()

        # Phase 3.5 - disable BotFather privacy mode so the bot sees all
        # group/topic messages (otherwise it only sees mentions/commands).
        if not is_at_least(snapshot.state, InstallState.BOT_PRIVACY_OFF):
            await disable_privacy(client, bot_username=snapshot.data["bot_username"])
            store.set(InstallState.BOT_PRIVACY_OFF)
            append_log(paths, "bot privacy mode disabled")
            snapshot = store.load()

        # Phase 3.7 - Verify the supergroup we think we own still exists on
        # Telegram. Soft "delete chat for me" / lost local cache resolve
        # themselves transparently because create_supergroup_chat(force=False)
        # is a server hit. Only when Telegram itself reports the id as gone
        # do we roll the install state back, blank the Doppler IDs, and let
        # phase 4 below recreate from scratch.
        stored_sg_id = snapshot.data.get("supergroup_id")
        stored_chat_id = snapshot.data.get("supergroup_chat_id")
        if stored_sg_id is not None:
            alive = await supergroup_exists(
                client,
                supergroup_id=int(stored_sg_id),
                chat_id=int(stored_chat_id) if stored_chat_id else None,
            )
            if not alive:
                console.log(
                    f"[yellow]supergroup {stored_sg_id} is gone from Telegram; "
                    f"recreating[/yellow]"
                )
                snap = store.load()
                for k in (
                    "supergroup_chat_id",
                    "supergroup_id",
                    "supergroup_title",
                    "is_forum",
                    "machine_topic",
                ):
                    snap.data.pop(k, None)
                snap.state = InstallState.BOT_PRIVACY_OFF
                store.save(snap)
                snapshot = store.load()
                writeback["ARBOS_TELEGRAM_CHAT_ID"] = ""
                writeback["ARBOS_TELEGRAM_SUPERGROUP_ID"] = ""
                inputs.supergroup_chat_id = None
                inputs.supergroup_id = None
                append_log(
                    paths,
                    f"supergroup {stored_sg_id} gone; resetting to BOT_PRIVACY_OFF",
                )

        # Phase 4 - Supergroup (adopt from Doppler if known, else create)
        if not is_at_least(snapshot.state, InstallState.SUPERGROUP_CREATED):
            if inputs.supergroup_chat_id is not None:
                sg = await adopt_supergroup(
                    client,
                    chat_id=inputs.supergroup_chat_id,
                    supergroup_id=inputs.supergroup_id,
                )
                append_log(paths, f"supergroup adopted: chat_id={sg.chat_id}")
            else:
                sg = await create_supergroup(client, title=inputs.supergroup_title)
                writeback["ARBOS_TELEGRAM_CHAT_ID"] = str(sg.chat_id)
                writeback["ARBOS_TELEGRAM_SUPERGROUP_ID"] = str(sg.supergroup_id)
                append_log(paths, f"supergroup created: chat_id={sg.chat_id}")
            store.set(
                InstallState.SUPERGROUP_CREATED,
                supergroup_chat_id=sg.chat_id,
                supergroup_id=sg.supergroup_id,
                supergroup_title=sg.title,
                is_forum=sg.is_forum,
            )
            snapshot = store.load()

        # Phase 5 - forum mode (defensive, idempotent)
        if not is_at_least(snapshot.state, InstallState.FORUM_ENABLED):
            sg_id = snapshot.data["supergroup_id"]
            ok = await ensure_forum(client, supergroup_id=sg_id)
            store.set(InstallState.FORUM_ENABLED, is_forum=ok)
            append_log(paths, f"forum enabled: supergroup_id={sg_id}")
            snapshot = store.load()

        # Phase 6 - bot admin (idempotent)
        if not is_at_least(snapshot.state, InstallState.BOT_ADDED):
            membership = await add_and_promote(
                client,
                chat_id=snapshot.data["supergroup_chat_id"],
                bot_username=snapshot.data["bot_username"],
            )
            store.set(InstallState.BOT_ADDED, bot_user_id=membership.bot_user_id)
            append_log(
                paths,
                f"bot added+promoted: bot_user_id={membership.bot_user_id}",
            )
            snapshot = store.load()

        # Phase 6.5 - Verify the per-machine topic still exists. If it was
        # deleted from the supergroup (or never persisted to Telegram), roll
        # state back to BOT_ADDED so the phase 7 block below re-resolves it
        # (reuse-by-name first, then fresh create).
        topic_data = snapshot.data.get("machine_topic", {})
        stored_topic_id = topic_data.get("topic_id")
        if (
            is_at_least(snapshot.state, InstallState.TOPICS_CREATED)
            and stored_topic_id is not None
        ):
            chat_id = snapshot.data["supergroup_chat_id"]
            if not await topic_exists(
                client, chat_id=chat_id, topic_id=stored_topic_id
            ):
                console.log(
                    f"[yellow]topic {topic_data.get('name')!r} "
                    f"(id={stored_topic_id}) gone; re-resolving[/yellow]"
                )
                snap = store.load()
                snap.data.pop("machine_topic", None)
                snap.state = InstallState.BOT_ADDED
                store.save(snap)
                snapshot = store.load()
                topic_data = {}
                append_log(
                    paths,
                    f"topic {stored_topic_id} gone; resetting to BOT_ADDED",
                )

        # Phase 7+8 - single per-machine topic + welcome. create_machine_topic
        # adopts an existing same-named topic when present (is_new=False), so
        # we only post the welcome on a true creation.
        if not is_at_least(snapshot.state, InstallState.TOPICS_CREATED):
            chat_id = snapshot.data["supergroup_chat_id"]
            topic = await create_machine_topic(
                client,
                chat_id=chat_id,
                machine_name=inputs.machine_name,
            )
            if topic.is_new:
                topic = await seed_welcome(client, chat_id=chat_id, topic=topic)
            else:
                topic.welcome_message_id = topic_data.get("welcome_message_id")
            store.set(
                InstallState.TOPICS_CREATED,
                machine_topic={
                    "name": topic.name,
                    "topic_id": topic.topic_id,
                    "welcome_message_id": topic.welcome_message_id,
                },
            )
            append_log(paths, f"machine topic ready: {topic.name} -> {topic.topic_id}")
            snapshot = store.load()

    # Phase 9 - persist + writeback (no live arbos client needed)
    if snapshot.state != InstallState.COMPLETE:
        topic_data = snapshot.data.get("machine_topic", {})
        machine_topic = CreatedTopic(
            name=topic_data["name"],
            topic_id=topic_data["topic_id"],
            welcome_message_id=topic_data.get("welcome_message_id"),
        )
        write_secrets(
            paths,
            api_hash=inputs.telegram_api_hash,
            bot_token=snapshot.data["bot_token"],
        )
        stored = build_stored_config(
            paths=paths,
            inputs=inputs,
            bot_username=snapshot.data["bot_username"],
            bot_token=snapshot.data["bot_token"],
            bot_user_id=snapshot.data["bot_user_id"],
            supergroup_chat_id=snapshot.data["supergroup_chat_id"],
            supergroup_id=snapshot.data["supergroup_id"],
            supergroup_title=snapshot.data.get("supergroup_title", inputs.supergroup_title),
            is_forum=bool(snapshot.data.get("is_forum", True)),
            machine_topic=machine_topic,
        )
        write_final_config(paths, stored)
        store.set(InstallState.COMPLETE)
        append_log(paths, "install complete")

    # Always emit a writeback file (may be empty); run.sh consumes it.
    write_doppler_writeback(paths, writeback)

    console.rule("[bold green]install complete[/bold green]")
    console.print(f"storage:      {paths.arbos}")
    console.print(f"config:       {paths.config_path}")
    console.print(f"machine:      {inputs.machine_name}  bot: @{inputs.bot_username}")
    if writeback:
        console.print(
            f"writeback: {len(writeback)} key(s) queued for doppler in {paths.writeback_path}"
        )


@app.command()
def install(
    verbose: bool = typer.Option(False, "--verbose", "-v"),
) -> None:
    """Run (or resume) the per-machine bootstrap installer."""
    _setup_logging(verbose)
    paths = InstallPaths.discover()
    try:
        inputs = _build_inputs()
    except Exception as exc:
        console.print(f"[red]invalid inputs:[/red] {exc}")
        raise typer.Exit(code=2)

    try:
        asyncio.run(_run_install(paths, inputs))
    except InstallError as exc:
        console.print(f"[red]install failed:[/red] {exc}")
        raise typer.Exit(code=1)
    except KeyboardInterrupt:
        console.print("[yellow]interrupted[/yellow]")
        raise typer.Exit(code=130)

    # Force a hard exit to bypass Python's atexit handlers. On macOS
    # OPENSSL_cleanup() races with libtdjson's still-running worker threads
    # and segfaults during teardown -- after our work has already succeeded.
    # Flush user-visible streams so nothing is lost.
    import sys as _sys
    _sys.stdout.flush()
    _sys.stderr.flush()
    os._exit(0)


@app.command()
def status() -> None:
    """Print the persisted install state for this host's ``~/.arbos``."""
    paths = InstallPaths.discover()
    if not paths.state_path.exists():
        console.print(f"[yellow]no install state at {paths.state_path}[/yellow]")
        raise typer.Exit(code=1)

    snapshot = StateStore(paths.state_path).load()
    table = Table(show_header=False, box=None)
    table.add_row("arbos_dir", str(paths.arbos))
    table.add_row("state", snapshot.state.value)
    table.add_row("updated_at", snapshot.updated_at or "-")
    table.add_row("last_error", snapshot.last_error or "-")
    for k in (
        "bot_username",
        "bot_user_id",
        "supergroup_chat_id",
        "supergroup_id",
    ):
        if k in snapshot.data:
            table.add_row(k, str(snapshot.data[k]))
    if "machine_topic" in snapshot.data:
        mt = snapshot.data["machine_topic"]
        table.add_row("machine_topic", f"{mt.get('name')} (id={mt.get('topic_id')})")
    console.print(table)


@app.command("privacy-off")
def privacy_off(
    verbose: bool = typer.Option(False, "--verbose", "-v"),
) -> None:
    """One-shot retrofit: drive BotFather /setprivacy -> Disable for an
    already-installed bot. Idempotent."""
    _setup_logging(verbose)
    paths = InstallPaths.discover()
    try:
        inputs = _build_inputs()
    except Exception as exc:
        console.print(f"[red]invalid inputs:[/red] {exc}")
        raise typer.Exit(code=2)

    async def _go() -> None:
        paths.bootstrap()
        store = StateStore(paths.state_path)
        snapshot = store.load()
        bot_username = snapshot.data.get("bot_username") or inputs.bot_username

        async with open_session(inputs, paths) as client:
            await disable_privacy(client, bot_username=bot_username)
            if snapshot.state == InstallState.COMPLETE:
                # mark idempotently in the data blob (don't move state)
                snap = store.load()
                snap.data["privacy_disabled"] = True
                store.save(snap)
            elif not is_at_least(snapshot.state, InstallState.BOT_PRIVACY_OFF):
                store.set(InstallState.BOT_PRIVACY_OFF)
        console.print(f"[green]privacy mode disabled for @{bot_username}[/green]")

    try:
        asyncio.run(_go())
    except InstallError as exc:
        console.print(f"[red]privacy-off failed:[/red] {exc}")
        raise typer.Exit(code=1)

    import sys as _sys
    _sys.stdout.flush()
    _sys.stderr.flush()
    os._exit(0)


agent_app = typer.Typer(
    add_completion=False,
    help="Per-machine always-on agent (PM2 entrypoint).",
)
app.add_typer(agent_app, name="agent")


@agent_app.command("run")
def agent_run(
    verbose: bool = typer.Option(False, "--verbose", "-v"),
) -> None:
    """Long-running pong agent. Foreground; intended for PM2 to supervise."""
    _setup_logging(verbose)
    from .agent import run_agent

    paths = InstallPaths.discover()
    try:
        rc = asyncio.run(run_agent(paths))
    except KeyboardInterrupt:
        console.print("[yellow]interrupted[/yellow]")
        raise typer.Exit(code=130)
    raise typer.Exit(code=rc)


@app.command()
def reset(
    hard: bool = typer.Option(False, "--hard", help="Actually delete .arbos/."),
) -> None:
    """Remove ``.arbos/`` (irreversible). Requires ``--hard`` and typed confirmation."""
    paths = InstallPaths.discover()
    if not hard:
        console.print(f"would remove {paths.arbos} (re-run with --hard to confirm)")
        return
    if not paths.arbos.exists():
        console.print(f"[yellow]nothing at {paths.arbos}[/yellow]")
        return
    typed = typer.prompt(f"Type 'yes' to delete {paths.arbos}", default="")
    if typed.strip().lower() != "yes":
        console.print("[yellow]aborted[/yellow]")
        raise typer.Exit(code=1)
    shutil.rmtree(paths.arbos)
    console.print(f"[green]removed {paths.arbos}[/green]")


def main() -> None:
    """Entry point for ``python -m arbos``."""
    app()


if __name__ == "__main__":
    main()

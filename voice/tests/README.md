# Local voice tests

From `voice/`, create a virtual environment and install the project with its test extra:

```sh
python3 -m venv .venv
. .venv/bin/activate
python -m pip install -e '.[test]'
python -m pytest tests/test_updates.py tests/test_usage_loop.py tests/test_worker_lifecycle.py tests/test_vision_consent.py -q
```

These tests use injected local SDK effects and synthetic frames. They need the SDK imports, but do not join a room, call a provider, download a model or require API keys. Use this explicit file list: the directory also contains opt-in live/evaluation scripts with different prerequisites.

The test extra declares compatible pytest/pytest-asyncio floors verified with Python 3.14.2, pytest 8.4.1 and pytest-asyncio 1.4.1.post1. The inspected SDK cohort was LiveKit Agents/OpenAI/Silero 1.8.8. Runtime dependency declarations remain unchanged; this is not a production dependency lock or a live-call/device calibration result.

The generated transition laws need no voice SDK or configuration. From the repository root, install the frozen harness dependencies and run its suite:

```sh
pnpm --dir harness install --frozen-lockfile --ignore-scripts
pnpm --dir harness --filter arbos-pi-ext test
```

For just the voice laws, run `node --test voice-state-laws.test.ts` from `harness/arbos-pi-ext`. The isolated Python bridge imports only the pure state modules and exercises six seeded laws with 256 cases each.

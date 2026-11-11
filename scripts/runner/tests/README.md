# Isolated Mac lease checks

On macOS, from the repository root:

```sh
python3 scripts/runner/tests/test_slot_lifecycle.py
```

The stdlib suite copies the three actual controllers into fresh temporary trees.
Only those copies run. It validates its path adapters before replacing default
locations, then supplies a clean environment and synthetic files, PIDs and
commands. macOS sandboxing denies network, reads/writes under `/Users/Shared`,
and execution of real xcrun, sudo, open and osascript. No runner lock, simulator,
GUI application, provider, release or deployment is used. Signals target only
the suite's own controller subprocesses.

The real macOS lockf is exercised against fixture-only mutex files, including
concurrent exclusion and releasing the mutex before a queue sleep. The no-lock
primitive case must fail closed. A local stdlib flock shim checks the Linux
adapter's invocation; this is not a claim of running the Linux executable here.
The controllers use native macOS lockf or Linux flock; unsupported environments
without either cannot grant a lease. The GUI's stat adapter supports BSD/GNU
formats. The operational scripts' simulator/GUI consumers remain Mac-specific.

The suite requires Python 3, `/bin/bash`, macOS sandbox-exec and lockf; it installs
nothing and deliberately does not silently skip unsupported host checks. Runtime
controllers add no package dependency. Existing priority, lease, polling and
reclamation thresholds remain configurable through the same variables.

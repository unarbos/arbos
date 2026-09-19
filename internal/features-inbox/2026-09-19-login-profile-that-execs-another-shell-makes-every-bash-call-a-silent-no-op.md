---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# A login profile that `exec`s another shell makes every bash call a silent no-op

Found by the features agent's own probe, 2026-09-19, on `main` `587515fd`.
A machine we did not choose: a `~/.bash_profile` that ends in `exec zsh`
(the common way to get zsh on a machine where `chsh` is not allowed —
corporate Linux, older Macs, shared hosts).

## What happens

The kernel runs every `bash` tool call, every reproduction re-run and the
environment probe as a login shell — `bash -lc <script>` — so the person's
profile puts their conda env, venv or toolchain on PATH (SWE-bench images
keep their interpreter that way). With `exec <other shell>` in the profile:

```
$ printf 'exec sh\n' > $H/.bash_profile
$ HOME=$H bash -lc 'echo hi; exit 3' </dev/null; echo exit=$?
exit=0
```

Nothing printed, exit 0: the profile replaced bash before `-c` ran, the
other shell read an empty stdin and left. On such a machine:

- every `bash` tool call reports success with empty output and the command
  never ran — a misreport on every step, and a model that reads "ran, no
  output" as done;
- `changes`' reproduction re-runs "pass" (exit 0) without running;
- the environment probe finds "no python on PATH" and writes that into the
  prompt.

`ARBOS_NO_LOGIN_SHELL=1` turns the login shell off, but nobody on such a
machine knows to set it, and nothing tells them.

A profile that merely prints (a banner, `neofetch`, a conda message) puts
that text at the top of every tool result; that is the same on Cursor and
is not this finding.

## What should hold

The kernel probes the login shell once (`bash -lc 'printf <marker>'`, a
few seconds' timeout). If the marker does not come back, commands run
without the login profile (`bash -c`), and the kernel says so once on the
main chat and in the log: what the profile did, that PATH additions made
only there are not seen, and the two ways out (`ARBOS_NO_LOGIN_SHELL=1` to
keep it that way quietly, or move the `exec` behind an interactive check).

Fixed in the same cycle: [#768](https://github.com/unarbos/arbos/pull/768).

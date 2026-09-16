# qa-034: the git guard judges `cd toy-repo && git commit …` in the place, not in toy-repo

- Feature: git guard (#214), `main` @ `318b57ae`
- Severity: medium: any worker that changes directory in the same command before committing is refused for a missing identity the repository has, and asks the user for an author (kickoff item 8 partial).
- Rollout: `internal/qa/rollouts/20260915T033931Z-kickoff-session/`, worker `run-and-fix-toy-repo`
- Status: fix PR [#217](https://github.com/unarbos/arbos/pull/217) (`check()` follows `cd`/`pushd` segments; test red without the fix)

## Repro

Place root is not a git repository; `toy-repo/` inside it is, with `user.name`/`user.email` set. Run `bash cwd=. command="cd toy-repo && git add hello.py && git commit -m fix"`.

## Expected

The commit is judged in `toy-repo`: identity present, branch check against toy-repo's branch.

## Actual

`git guard: <place>/./ has no user.name or user.email configured, so this commit would be attributed to nobody …` — the segment's `cwd` (the place) was checked; the `cd toy-repo` segment was skipped by `GitCall::parse`.

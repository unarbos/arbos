# Image activation lease and verifier hardening (frozen)

Private4-file follow-up applied AFTER frozen conditional-publication patch. PATCH/FROZEN-SOURCE/GATES pin exact source. No deployment/service/GitHub/git/configuration/secret action executed; only generated files, real local FD/queue operations and synthetic collaborators. Parent owns publication.

## Actual reproduced gap and fix

The H7 core lease ended after run_canary_smokes. The actual roll_image activation slice then changed config/default and could notice/restart the relay outside any lease. BEFORE.json/log pins2semanticRED (hot200 default POST and404 fallback restart overlap an independently granted canary) plus1positivePASS (restart failure still blocks public default receipt). Actual source and trace copies are retained in before/. An initial shell-only assertion was insufficient; canonical RED uses direct CLI errexit context plus an independent Python observation assertion.

activate_image_default now performs smoke→config/default POST→existing non200 notice/restart/grace/health check→public default marker as one callback under the existing descriptor lease. Build/registration precede it; box rollout follows it. Before config/default mutation, a short queue transaction advances the agentd revision, invalidating older automatic decisions even before its public default marker exists. No HTTP/git/network while queue lock is held; no second lifecycle machine. HTTP fallback policy, notice/drain/restart grace values, rc1 image hold and rc2 inconclusive policy stay. Critical config/restart failures propagate explicitly from the if-called callback; external75 stays normalized to operational76.

Current actual activation5tests PASS: both overlap regressions, restart failure, pre-default receipt invalidation and revision failure before mutation. All22core+19publication adapter groups pass too (46total), worker Bash syntax PASS, exact4source hashes unchanged. Pure helper/bridge/TS are byte-identical to conditional-publication: reuse its4laws1024cases+protocol/4actual-function mutants with matching helper SHA instead of pretending another semantic change occurred there.

## Verification mechanics retained

Intermediate verifier clauses now explicitly exit on failure rather than relying on Bash errexit through if-called functions. Actual production bodies remain extracted unchanged; only owned test assertions/effect collaborators changed. The isolated lane/smoke suite substitutes the activation effect; the activation suite overrides it with the actual full callback. Fragment extraction handles the moved default body. A cancelled background wrapper is not mistaken for its still-live leased callback: after releasing the fixture barrier, the check waits boundedly for actual marker cleanup. This is a fixture synchronization correction, not a new production cleanup workaround.

The full fallback is exercised with actual selected source and synthetic config/secret/HTTP/service effects. Synthetic bearer text is never a real credential. The test fixture allows writes only inside its canonical temporary root and retires its own process group. No whole-worker top-level/.env execution is involved. Kernel lease lifetime is local process ownership; no remote transaction or guaranteed external cleanup is claimed.

## Handoff order

1. H7 core commit ee1ee4abd / corrected patch3d9d50bc…
2. deploy-publication/conditional-publication.patch05a535a0…
3. This activation-lease.patch (hash in PATCH.json)

Independent bounded review is BASELINE_RUST_REVIEW.md. Parent should rerun the conventional scripts on the composed source and retain existing Linux CI/full selftest as separate gates. Never install/deploy as part of this contribution task.

# SQLite scale results

Warm local source-SQL comparison; logical visits are measured NVISIT, not physical reads. All timings below are milliseconds.

| Axis | Scale | Group full-state p50/p95/p99 | Position-only p50/p95/p99 | Paired saved median95%CI | Visits before→after |
|---|---:|---:|---:|---:|---:|
| history | 1× | 0.017/0.020/0.022 | 0.004/0.005/0.006 | 0.013–0.013 | 102→1 |
| history | 10× | 0.049/0.054/0.059 | 0.004/0.005/0.005 | 0.044–0.045 | 1002→1 |
| history | 100× | 0.380/0.400/0.413 | 0.006/0.008/0.015 | 0.373–0.376 | 10002→1 |
| history | 1000× | 5.983/6.179/6.209 | 0.031/0.043/0.046 | 5.937–5.981 | 100002→1 |
| registry | 1× | 0.017/0.018/0.019 | 0.004/0.005/0.006 | 0.013–0.013 | 102→1 |
| registry | 10× | 0.018/0.023/0.040 | 0.004/0.006/0.010 | 0.013–0.014 | 102→1 |
| registry | 100× | 0.017/0.020/0.022 | 0.004/0.005/0.006 | 0.013–0.013 | 102→1 |
| registry | 1000× | 0.017/0.019/0.023 | 0.004/0.005/0.008 | 0.013–0.013 | 102→1 |

Both DM and group position comparisons at all8axis/scale combinations (16comparisons) have positive paired-median95% intervals. Maximum exact two-sided sign-test p=8.05e-29; Bonferroni-adjusted across16 comparisons ≤1.29e-27. 101randomizedpairs/comparison;2000bootstrapresamples. One comparison contains a non-improving pair; no claim of every pair improving.

The unchanged registry control grows from94µs at10rooms to90.845ms at10,000rooms (before-label p50); at1000× it executes10,001 statements, visits70,000 logical rows and returns60,000 SQL rows. The same SQL after-label p50 is90.971ms; paired saved95%CI −0.191–0.345ms, p=0.383. This separately exposes the unbounded N+1 path; no fix or improvement is claimed.

The fixed10-room history control stays near93–95µs. History1000× removes100,002→1logical visits and700,049→15VMsteps from each position check. Cache position metadata/default equivalence passes for both existing room kinds and missing rooms.

All8new regression tests pass0fail/skip (initial8.138s; final6.028s); before-suite-test count is N/A because this suite is new. Fixture/profile validation passes at all8scales. Initial21sample pre-editpilot is preserved but excluded after correcting its provisional relationships. Both final arms use the same corrected fresh fixture and actual before/after SQL.

See README for synthetic assumptions, engine versions, readonly boundary and blocked native/otherDB coverage. Largest temporary fixture177,668,096B; files deleted between scales. Compiled engine1,341,688B. SQL-only Ccolumnconsumption omits Rustmapping, native/HTTP/CRDT and concurrency. Scanstatus instrumentation overhead remains; observed1µstimerquantization. Charts use milliseconds, p50lines with p50→p95shading. No user-visible chat latency claim.

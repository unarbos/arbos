# Conditional deployment publication review

Scoped source review: PASS, no actionable defect found in the four recorded files. Read-only; no tests/services/deployments executed by this reviewer. The author’s test results remain separate evidence.

- `deploy_queue_transaction` serializes observer, conditional comparison, revision update and queue mutation under the same short FD8 lock. The hook re-reads the complete receipt immediately before publication, after the caller’s external ancestry/path decision. Manual two-argument arrival order remains explicit.
- The receipt binds kind, bounded monotonic revision, queued SHA/absence, installed SHA/absence and image-default SHA/absence. All queue publish/ack paths advance revision before mutation. Absent→publish→ack→absent therefore refuses an older receipt. A failed revision write prevents mutation; a failed later rename/unlink leaves a conservative newer revision. No wrap/reset fallback is present.
- Missing legacy revision is initialized under the lock; existing malformed, symlinked, overlong or exhausted revision is refused. Strict queue/receipt grammars reject unknown/duplicate fields, invalid kinds and mixed output. Installed/default missing files are distinguished from inaccessible parents or malformed/symlinked files.
- Both automatic callers retain the observed receipt across their decision. Worker only treats explicit75 as a changed decision; poller observation failure/malformed output has no unconditional fallback. Existing on-main/CI guards are retained.
- This is admission-time comparison, not a filesystem power-loss transaction or a remote CI/repository snapshot. Installed/default changes are re-read by value; arbitrary out-of-contract writers bypassing the helper are not covered. No new image-default fallback/restart-lease repair is claimed here.

Reviewed source hashes:

```json
{
  "scripts/deploy/deploy-state.sh": "58cec985ecbd03a2e2fc034a04a8a651c952fb1ecb829544855113c42c681824",
  "scripts/deploy/arbos-deploy-hook": "a7d5981e53822f07c69835681ecd172a765bb62f52d31cfca2b402bc5b8f0e19",
  "scripts/deploy/arbos-deploy-worker": "8dd898e346d9b9e80bf06150da3e1e5fc8567ab8970fa63ba2b2ac9ad348c45b",
  "scripts/feedback/feedback_poller.py": "f68edb1e2b97e968ab1a8184c0ae91c43a3cd7d03ee81772e5df27bea77ddf20"
}
```

# Image default activation lease review

Scoped read-only review PASS; no additional actionable defect found in the frozen worker delta. No tests, configuration, services or deployment effects were run by this reviewer.

`wait_for_relay_lane` passes the entire actual activation callback to deploy_lane_run. The callback includes both smoke gates, revision admission, local config/default POST, existing non200 fallback planned restart + service restart + grace/health probe, and final public rolling SHA. The original after-lease rollout remains outside. Therefore a successful or failed default/restart attempt retains the same kernel lane through its real conflicting effects.

Revision is advanced under only the short queue transaction before config/default effects; no HTTP is run while that queue lock is held. Invalid revision aborts before mutation. Activation callback explicitly checks config write, planned restart, service restart and final metadata writes because caller conditional context suppresses errexit. Nonzero reserved75 is normalized before returning as operational failure, keeping Deferred distinct. Existing fallback policy/grace/smoke0/1/2 behavior remains; this review does not endorse a new HTTP policy or claim remote rollback.

The actual roll_image slice is tested, not a duplicate model of call ordering. Competing real local FD-lane callers are injected in hot POST and fallback restart; tests require no overlap, final metadata and later lane release. Expected-failure fixtures assert exact exit and absence of receipt/default side effects, so missing effects cannot masquerade as a pass. Private helper verification is strengthened separately.

Hashes:
```json
{
  "scripts/deploy/arbos-deploy-worker": "07afc1fe02ec69d0d311074257e8d3b0b2bb7c446b56fd571025bd95f5974d3b",
  "scripts/deploy/test_deploy_lanes.py": "3f426c4125e26567e8751dc479d6eab68a7607b9f3126f7462473a841289a66a",
  "scripts/deploy/test_image_activation.py": "86b9db6c59ba982a3630c21686b37279ad325af57727aec6a1762f52fb77ffe3"
}
```

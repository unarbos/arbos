---
cursor:
  subagentId: "bc-08d8261b-fea2-5075-9949-d45f6f9d4acc"
---

> Rebuilt 2026-09-16 12:50 UTC as one page, from context, after `internal/mobile-mac-host.md` and `internal/mobile-testflight.md` were lost (see `internal/mobile-store-loss-2026-09-16.md`). Ids the context did not keep are marked so.

# The loop's Mac host and the TestFlight pipeline — essentials

## EC2 Mac (tag `arbos-mobile`)

- Region `us-east-1`, dedicated host `mac2-m2.metal` (Apple silicon), allocated 2026-09-15; instance public IP `3.89.43.68`, user `ec2-user`, key `/tmp/mobile/arbos-mobile.pem` on the loop's VM (host id / instance id: not retained in context — `aws ec2 describe-hosts --filters Name=tag:Name,Values=arbos-mobile`).
- Price ≈ $0.88/h, ≈ $21/day; 24-hour minimum on the host. Running cost at 12:50 UTC 09-16 ≈ **$24**. Rule: stop the instance when idle over two hours; release the host only when Jacob says. Recommendation on file (given 09-16 ~10:30): release it and run on CI + Jacob's phone once the simulator work is no longer the bottleneck.
- Installed: Xcode 27.0 with the iOS 27.0 simulator ("Arbos iPhone 15 Pro", udid `B1185668-…`), `idb`, `ffmpeg`, Pillow, `op` (service-account token in `~/.op-env`), `~/arbos` checkout, a local `arbos-hub` on `127.0.0.1:7780` with two replay kernels (`longproj`, `otherproj`) for link-loss tests, scripts `~/mac-cycle.sh`, `~/mac-voice.sh`, `~/mac-isolate-*.sh`, `~/mac-cycle13.sh`, `~/asc-feedback.py`, `~/find_row.py`.
- Security group: SSH + VNC from the VM's NAT range only.

## TestFlight

- App Store Connect app `6812503407`, bundle `com.unarbos.arbos.ios`, team `25SCF3Q2AK`, version 0.2.0. Build number = `git rev-list --count HEAD` on `main` (994 = #306).
- Key `ZV82D3ZWRT` ("Arbos IOS", Admin, cloud-managed distribution allowed) — vault only, never printed. CI secrets `IOS_ASC_KEY_P8`, `IOS_ASC_KEY_ID`, `IOS_ASC_ISSUER_ID`, `IOS_SECRETS_PLIST`.
- `.github/workflows/ios-testflight.yml`: on every `ios/**` commit to `main` — bake `Secrets.plist`, `check-secrets.sh` guard (pod/ArbosLife hub only, never local; `client-phone` token accepted by the hub), archive with cloud signing, upload. Push entitlement added when the App ID has the capability.
- Jacob is an internal tester; builds arrive by themselves. Beta feedback is read by `~/asc-feedback.py` every 15 minutes.

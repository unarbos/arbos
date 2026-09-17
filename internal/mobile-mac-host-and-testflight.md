---
cursor:
  subagentId: "bc-08d8261b-fea2-5075-9949-d45f6f9d4acc"
---

> Rebuilt 2026-09-16 12:50 UTC as one page, from context, after `internal/mobile-mac-host.md` and `internal/mobile-testflight.md` were lost (see `internal/mobile-store-loss-2026-09-16.md`). Ids the context did not keep are marked so.

# The loop's Mac host and the TestFlight pipeline — essentials

## EC2 Mac (tag `arbos-mobile`)

- Region `us-east-1`, dedicated host `h-0f61194598b26ed71` (`mac2-m2.metal`, Apple silicon, `us-east-1a`), allocated 2026-09-15. Root disk 250 GB gp3. User `ec2-user`.
- **Access, rebuilt 2026-09-17 (cycle 41).** The original key pair `arbos-mobile` was created on 2026-09-15 and its private half was kept only in `/tmp` on the first worker's VM. That VM is gone, so the key is gone: it is not in the vault, not on ArbosLife, not in SSM or Secrets Manager, and EC2 Mac instances take neither SSM nor EC2 Instance Connect. Instance `i-0423fc98fadf937c0` answered on 22 but refused every key we hold.
  - Replacement key pair `arbos-mobile-2` (ed25519). **The private key now lives in the vault**, item `l6zmhcj7thvr6bi7xdfr36akc4`, a DOCUMENT named `arbos-mobile-2.pem`. Read it with `op document get l6zmhcj7thvr6bi7xdfr36akc4 --vault Arbos --out-file <0600 path>`; never print it. Any worker can now get in from a cold start.
  - Recovery AMI `ami-0b2f6406037f0a8cb` holds the whole disk as it stood on 2026-09-17 03:15 UTC (Xcode 27, the iOS 27 simulator, `idb`, `ffmpeg`, `~/arbos`, the loop scripts, `~/mobile-out`, `~/mobile-docs`). The old root volume `vol-0a54824c1f1b15d92` is kept as a second copy, tagged `arbos-mobile-root-preserve`, with `DeleteOnTermination` off. Relaunch from the AMI onto the host with `--placement HostId=h-0f61194598b26ed71,Tenancy=host --key-name arbos-mobile-2`; `ec2-macos-init` writes the new public key into `~ec2-user/.ssh/authorized_keys` because the instance id is new.
  - **Reaching it from a cloud VM:** this VM's NAT address rotates over a pool, so a `/32` in the security group goes stale within minutes. Jump through ArbosLife instead, which has a fixed address: `ssh -J const@204.12.171.6 ec2-user@<mac ip>`, both hops on keys from the vault. `204.12.171.6/32` is allowed on 22 and 5900.
- Price ≈ $0.88/h, ≈ $21/day; 24-hour minimum on the host. Running cost at 12:50 UTC 09-16 ≈ **$24**. Rule: stop the instance when idle over two hours; release the host only when Jacob says. Recommendation on file (given 09-16 ~10:30): release it and run on CI + Jacob's phone once the simulator work is no longer the bottleneck.
- Installed: Xcode 27.0 with the iOS 27.0 simulator ("Arbos iPhone 15 Pro", udid `B1185668-…`), `idb`, `ffmpeg`, Pillow, `op` (service-account token in `~/.op-env`), `~/arbos` checkout, a local `arbos-hub` on `127.0.0.1:7780` with two replay kernels (`longproj`, `otherproj`) for link-loss tests, scripts `~/mac-cycle.sh`, `~/mac-voice.sh`, `~/mac-isolate-*.sh`, `~/mac-cycle13.sh`, `~/asc-feedback.py`, `~/find_row.py`.
- Security group: SSH + VNC from the VM's NAT range only.

## TestFlight

- App Store Connect app `6812503407`, bundle `com.unarbos.arbos.ios`, team `25SCF3Q2AK`, version 0.2.0. Build number = `git rev-list --count HEAD` on `main` (994 = #306).
- Key `ZV82D3ZWRT` ("Arbos IOS", Admin, cloud-managed distribution allowed) — vault only, never printed. CI secrets `IOS_ASC_KEY_P8`, `IOS_ASC_KEY_ID`, `IOS_ASC_ISSUER_ID`, `IOS_SECRETS_PLIST`.
- `.github/workflows/ios-testflight.yml`: on every `ios/**` commit to `main` — bake `Secrets.plist`, `check-secrets.sh` guard (pod/ArbosLife hub only, never local; `client-phone` token accepted by the hub), archive with cloud signing, upload. Push entitlement added when the App ID has the capability.
- Jacob is an internal tester; builds arrive by themselves. Beta feedback is read by `~/asc-feedback.py` every 15 minutes.

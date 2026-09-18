---
cursor:
  subagentId: "bc-08d8261b-fea2-5075-9949-d45f6f9d4acc"
---

> Rebuilt 2026-09-16 12:50 UTC as one page, from context, after `internal/mobile-mac-host.md` and `internal/mobile-testflight.md` were lost (see `internal/mobile-store-loss-2026-09-16.md`). Ids the context did not keep are marked so.

# The loop's Mac host and the TestFlight pipeline — essentials

## EC2 Mac (tag `arbos-mobile`)

- Region `us-east-1`, dedicated host `h-0f61194598b26ed71` (`mac2-m2.metal`, Apple silicon, `us-east-1a`), allocated 2026-09-15. Root disk 250 GB gp3. User `ec2-user`.
- **Live instance (from 2026-09-17 05:04 UTC): `i-016b41e8f552dec09`, `13.217.52.163`**, on the same dedicated host. The old `i-0423fc98fadf937c0` / `3.89.43.68` is terminated; that address is dead.
- **Access, rebuilt 2026-09-17 (cycle 41).** The original key pair `arbos-mobile` was created on 2026-09-15 and its private half was kept only in `/tmp` on the first worker's VM. That VM is gone, so the key is gone: it is not in the vault, not on ArbosLife, not in SSM or Secrets Manager, and EC2 Mac instances take neither SSM nor EC2 Instance Connect. The old instance answered on 22 and refused every key we hold.
  - Replacement key pair `arbos-mobile-2` (ed25519). **The private key now lives in the vault**, item `l6zmhcj7thvr6bi7xdfr36akc4`, a DOCUMENT named `arbos-mobile-2.pem`. Read it with `op document get l6zmhcj7thvr6bi7xdfr36akc4 --vault Arbos --out-file <0600 path>`; never print it. Any worker can now get in from a cold start.
  - Two recovery AMIs, both arm64_mac, both holding the full rig (Xcode 27, the iOS 27 simulator and the `Arbos iPhone 15 Pro` device, `idb`, `ffmpeg`, `~/arbos`, the loop scripts, `~/mobile-out`, `~/mobile-docs`): `ami-0b2f6406037f0a8cb` is the disk as it stood at 03:15 UTC, `ami-0c071a2eff7677a55` the same disk with the `arbos-mobile-2` key already in `authorized_keys`. Relaunch onto the host with `--placement HostId=h-0f61194598b26ed71,Tenancy=host,Affinity=host --key-name arbos-mobile-2`.
  - **Reaching it from a cloud VM:** this VM's NAT address rotates over a pool, so a `/32` in the security group goes stale within minutes. Jump through ArbosLife instead, which has a fixed address: `ssh -J const@204.12.171.6 ec2-user@<mac ip>`, both hops on keys from the vault. `204.12.171.6/32` is allowed on 22 and 5900.
  - **A rented Mac costs about two hours to swap, almost all of it waiting.** Stopping or terminating a `mac2` instance puts the dedicated host into a scrubbing workflow that ran 109 minutes on 09-17, and nothing can launch on the host until it ends. `create-replace-root-volume-task` does *not* scrub — it swaps the root disk of a running instance in about five minutes — so prefer it to a stop whenever the fix is on the disk. It only accepts a snapshot of a volume that was root on that same instance, but it accepts `--image-id` for any AMI with matching architecture and billing, and these Mac AMIs carry no billing product, so an AMI registered by hand from an edited snapshot is accepted.
  - **A macOS disk can be edited from Linux.** `apfs-fuse` reads it; the out-of-tree `linux-apfs-rw` module writes it, but only when mounted `-o vol=0,readwrite` — plain `rw` silently downgrades to read-only with "experimental writes disabled" in `dmesg`. Worth knowing, though on 09-17 it turned out to be unnecessary: `ec2-macos-init` provisions the launch key pair into `~ec2-user/.ssh/authorized_keys` by itself on a new instance id, and the hour spent proving otherwise was a client-side mistake (the `ssh_config` entry still named the old key file). **Check which key the client offers with `ssh -vvv` before concluding the server is wrong.**
- Price ≈ $0.88/h, ≈ $21/day; 24-hour minimum on the host. Running cost at 12:50 UTC 09-16 ≈ **$24**. Rule: stop the instance when idle over two hours; release the host only when Jacob says. Recommendation on file (given 09-16 ~10:30): release it and run on CI + Jacob's phone once the simulator work is no longer the bottleneck.
- Installed: Xcode 27.0 with the iOS 27.0 simulator ("Arbos iPhone 15 Pro", udid `B1185668-…`), `idb`, `ffmpeg`, Pillow, `op` (service-account token in `~/.op-env`), `~/arbos` checkout, a local `arbos-hub` on `127.0.0.1:7780` with two replay kernels (`longproj`, `otherproj`) for link-loss tests, scripts `~/mac-cycle.sh`, `~/mac-voice.sh`, `~/mac-isolate-*.sh`, `~/mac-cycle13.sh`, `~/asc-feedback.py`, `~/find_row.py`.
- Security group: SSH + VNC from the VM's NAT range only.

## TestFlight

- App Store Connect app `6812503407`, bundle `com.unarbos.arbos.ios`, team `25SCF3Q2AK`, version 0.2.0. Build number = `git rev-list --count HEAD` on `main` (994 = #306).
- Key `ZV82D3ZWRT` ("Arbos IOS", Admin, cloud-managed distribution allowed) — vault only, never printed. CI secrets `IOS_ASC_KEY_P8`, `IOS_ASC_KEY_ID`, `IOS_ASC_ISSUER_ID`, `IOS_SECRETS_PLIST`.
- `.github/workflows/ios-testflight.yml`: on every `ios/**` commit to `main` — bake `Secrets.plist`, `check-secrets.sh` guard (pod/ArbosLife hub only, never local; `client-phone` token accepted by the hub), archive with cloud signing, upload. Push entitlement added when the App ID has the capability.
- Jacob is an internal tester; builds arrive by themselves. Beta feedback is read hourly by `deploy/feedback/asc-feedback.py`, run from the loop's own VM rather than the Mac, so feedback survives the Mac being down.
- **The build on his phone is the steward's number, and this is the one place it is written.** It is **1831**, written by the steward. This loop does not state a build number of its own and keeps no tally of superseded ones: when the steward writes a newer number, replace this line. Numbers inside dated cycle reports are what was true that hour, not the current build.

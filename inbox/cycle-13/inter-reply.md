# Reply — Inter vs San Francisco on the AWS Mac (from the mobile worker, bc-08d8261b)

**Verdict: no rendered verdict yet — the desktop app produces no visible window on the AWS Mac's virtual display, so neither the SF baseline nor the Inter captures could be taken. From the font metrics alone: Inter reads about 7 % larger in lowercase than SF at the same size (x-height 15.3 px vs 14.2 px at 14 px @2x, cap height 20.4 vs 19.7), so if it looks bigger than Cursor's chat, the fix is prose at 13 px, where Inter's x-height (14.2 px) equals SF's at 14.**

## What happened on the Mac

- Console session: made (`arbosgui`, auto-login via `/etc/kcpassword`; the original `ec2-user` cannot log in at the login window — its Secure Token password is out of step with the `dscl` one).
- `main` at `371445e` built release (`make bundle` passes, 880/880). The app launches, writes `state.toml` (frame `[-38, 30, 1100, 678]` — a window *was* created and placed on the 1024×768 virtual display), spawns its kernel, opens the driver socket. But nothing is drawn: no window on the screen (VNC capture of the console shows the desktop and Dock only), and the driver never answers `hello` (its job loop runs on gpui's foreground executor, which never ticks). Main thread idles in `-[NSApplication run]`. Reads like gpui's Metal layer / display link not firing on the EC2 virtual display (1024×768, no Retina mode offered). Same result launched from the console user's tmux and via `open -n` (LaunchServices), fresh XDG, `ApplePersistenceIgnoreState` set (needed: without it the app hangs on `talagentd` in `finishLaunching`).
- Consequence: captures 00-sf-* and 01–08 are not possible on this machine until gpui renders here. Xcode's iOS simulator renders fine on the same display, so it is not a GPU absence.

## Measurements (from the font files: Inter 4.1 Regular as bundled by #266; SF = `/System/Library/Fonts/SFNS.ttf` from this Mac, macOS 26.6)

| | Inter Regular | SF (System Font) | Δ |
| --- | --- | --- | --- |
| cap height, em | 0.728 | 0.705 | +3.3 % |
| x-height, em | 0.546 | 0.508 | +7.5 % |
| cap height at 14 px @2x | **20.4 px** | **19.7 px** | +0.6 px |
| x-height at 14 px @2x | **15.3 px** | **14.2 px** | +1.1 px |
| cap height at 13 px @2x | 18.9 px | — | |
| x-height at 13 px @2x | **14.2 px** | — | = SF at 14 |
| ascender / descender, em | 0.969 / −0.241 | 0.967 / −0.211 | line box +3 % (descender) |

Reading: the cap heights nearly match (Jacob's cycle-11 stills measured SF caps at 21 px; Inter will measure ~20–21 too), but lowercase — most of a paragraph — is a step bigger in Inter. If the prose reads larger than Cursor's on the same screen, **set prose to 13 px in Inter** (x-height parity with SF 14) rather than change the face; the Worked/run lines and the pills can follow the same step. Inter's deeper descender also means a fixed 23 px line box has 0.8 px less air under the baseline at 2x — likely invisible.

## Not done

Captures 00-sf-prose/worked-lines/tabs-panel and 01–08 — blocked as above. The Linux rig can take Inter captures at 1x today; a Mac 2x pair needs either gpui rendering on the EC2 virtual display (worth one look at `displayplacer` modes / a Retina virtual mode) or a Mac with a real display.

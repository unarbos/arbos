#!/bin/bash
# First-boot setup of the rented EC2 Mac (runs on the Mac as ec2-user).
# Xcode itself needs an Apple ID: see the note it prints at the end.
set -u
export PATH="/opt/homebrew/bin:$PATH"
echo "== $(sw_vers -productName) $(sw_vers -productVersion) $(uname -m) $(sysctl -n hw.ncpu) cores $(( $(sysctl -n hw.memsize) / 1073741824 )) GB"
df -h / | tail -1

# The root volume is bigger than the AMI's 100 GiB: grow APFS to fill it.
PDISK=$(diskutil list physical external | grep -m1 -o 'disk[0-9]*' || true)
if [ -n "$PDISK" ]; then
  APFSCONT=$(diskutil list | grep -m1 'Apple_APFS Container' | grep -o 'disk[0-9]*$' || true)
  sudo diskutil repairDisk "$PDISK" >/dev/null 2>&1 <<<"y" || true
  [ -n "$APFSCONT" ] && sudo diskutil apfs resizeContainer "$APFSCONT" 0 >/dev/null 2>&1 || true
  df -h / | tail -1
fi

# Screen Sharing (VNC on 5900), a password for ec2-user, auto-login so
# the simulator has a console session.
if [ -f "$HOME/.mac-pass" ]; then
  PASS=$(cat "$HOME/.mac-pass")
  sudo /usr/bin/dscl . -passwd /Users/ec2-user "$PASS" 2>/dev/null || true
  sudo /System/Library/CoreServices/RemoteManagement/ARDAgent.app/Contents/Resources/kickstart \
    -activate -configure -access -on -users ec2-user -privs -all -restart -agent -menu >/dev/null 2>&1 || true
  sudo /System/Library/CoreServices/RemoteManagement/ARDAgent.app/Contents/Resources/kickstart \
    -configure -clientopts -setvnclegacy -vnclegacy yes -setvncpw -vncpw "$PASS" >/dev/null 2>&1 || true
  sudo sysadminctl -autologin set -userName ec2-user -password "$PASS" >/dev/null 2>&1 || true
  sudo pmset -a sleep 0 displaysleep 0 disksleep 0 >/dev/null 2>&1 || true
  echo "== screen sharing on, auto-login set"
fi

# Tools.
brew list xcodes >/dev/null 2>&1 || brew install xcodes aria2 >/dev/null 2>&1 && echo "== xcodes $(xcodes version 2>/dev/null)"
brew list ffmpeg >/dev/null 2>&1 || brew install ffmpeg >/dev/null 2>&1 && echo "== ffmpeg ok"
# idb drives the simulator's UI (taps, text) from a shell.
brew list idb-companion >/dev/null 2>&1 || (brew tap facebook/fb >/dev/null 2>&1; brew install idb-companion >/dev/null 2>&1) && echo "== idb-companion ok"
python3 -m pip install --user -q fb-idb 2>/dev/null || pip3 install --user -q --break-system-packages fb-idb 2>/dev/null || true

# The repo and the kernel.
[ -d "$HOME/arbos/.git" ] || git clone -q https://github.com/unarbos/arbos "$HOME/arbos"
(cd "$HOME/arbos" && git fetch -q origin && git log --oneline -1 origin/main)
mkdir -p "$HOME/arbos-kernel/bin" "$HOME/arbos-kernel/places"
if [ -f "$HOME/arbos-kernel-0.2.0-macos-arm64" ]; then
  install -m 755 "$HOME/arbos-kernel-0.2.0-macos-arm64" "$HOME/arbos-kernel/bin/arbos-kernel"
  xattr -d com.apple.quarantine "$HOME/arbos-kernel/bin/arbos-kernel" 2>/dev/null || true
  "$HOME/arbos-kernel/bin/arbos-kernel" --version 2>/dev/null || "$HOME/arbos-kernel/bin/arbos-kernel" help 2>&1 | head -2
fi

echo "== xcode: $(xcode-select -p 2>/dev/null || echo none)"
ls /Applications | grep -i xcode || echo "== no Xcode.app yet — needs an Apple ID: xcodes install --latest"

# Arbos.app, the disk image, and the signed + notarized release.
#
#   make bundle            Arbos.app, ad-hoc signed (opens on this Mac)
#   make dmg               Arbos-<version>-<arch>.dmg around it
#   make release           the same with a Developer ID, notarized, stapled
#   make verify            what Gatekeeper will say about the app and image
#
# Signing is inside-out: the kernel first, then the app around it, then the
# image. Every binary gets the hardened runtime and a secure timestamp — the
# two things notarization refuses without — plus bundle/entitlements.plist.
#
# Release needs two things only Apple can hand out, both kept out of the
# tree:  SIGN="Developer ID Application: <name> (<team>)" names the
# certificate in the login keychain, and notarization takes either a keychain
# profile (NOTARY_PROFILE, made once with `xcrun notarytool store-credentials`)
# or APPLE_ID + APPLE_TEAM_ID + APPLE_APP_PASSWORD (an app-specific password
# from appleid.apple.com, what CI uses). `.env.release` beside this file may
# export any of them.

VERSION  := $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
# Build number: commits on the branch, so two builds of one version sort.
BUILD    := $(shell git rev-list --count HEAD 2>/dev/null || echo 0)
SHA      := $(shell git rev-parse --short=12 HEAD 2>/dev/null || echo unknown)
ARCH     := $(shell uname -m)
LOGO     := bundle/icon.png
# The identifier the signature is designated by — read from the plist so the
# two cannot drift.
BUNDLE_ID := $(shell plutil -extract CFBundleIdentifier raw -o - bundle/Info.plist)
OUT      := target/bundle
APP      := $(OUT)/Arbos.app
MACOS    := $(APP)/Contents/MacOS
RES      := $(APP)/Contents/Resources
KERNEL   := ../target/release/arbos-kernel
ICON1024 := $(OUT)/AppIcon-1024.png
ICONSET  := $(OUT)/AppIcon.iconset
DMG      := $(OUT)/Arbos-$(VERSION)-$(ARCH).dmg
DMGVOL   := Arbos $(VERSION)
DMGSTAGE := $(OUT)/dmg
DMGRW    := $(OUT)/Arbos-rw.dmg
DMGMNT   := /Volumes/$(DMGVOL)
DMGBG    := $(OUT)/dmg-background@2x.png
ENTITLEMENTS := bundle/entitlements.plist
# The ten sizes an .icns wants. Each is also emitted at @2x, so 512 covers 1024.
SIZES    := 16 32 128 256 512
# Ad-hoc unless told otherwise. `release` refuses to run ad-hoc.
SIGN     ?= -
# Ad-hoc only: pin the designated requirement to the bundle identifier. Left
# to itself an ad-hoc signature is designated by its cdhash, so to TCC every
# rebuild is a new app and the microphone is asked for again. A certificate
# brings its own stable requirement and leaves this out.
# $(call CODESIGN,<identifier>) — the kernel is a separate code object with
# its own identifier, so its designated requirement names that.
REQ       = $(if $(filter -,$(SIGN)),--requirements '=designated => identifier "$(1)"',)
CODESIGN  = codesign --force --options runtime --timestamp --entitlements $(ENTITLEMENTS) --identifier "$(1)" $(call REQ,$(1)) --sign "$(SIGN)"
# 32 bytes of FinderInfo carrying one bit: 0x0400, the custom-icon flag, at the
# flags field a folder keeps eight bytes in. `SetFile -a C` is the usual way and
# wants Xcode; this wants nothing.
CUSTOMICON := 0000000000000000040000000000000000000000000000000000000000000000
# Finder layout of the mounted image (icon positions, window size, backdrop).
# Runs through Finder, so the caller needs Automation permission for it; set
# DMG_LAYOUT=0 to skip on a headless box — the image still works, it is only
# less pretty.
DMG_LAYOUT ?= 1

-include .env.release

.PHONY: build bundle icon dmg notarize release verify open clean

build:
	cargo build --release
	cargo build --release -p arbos-kernel --manifest-path ../Cargo.toml

bundle: build
	rm -rf $(APP) $(ICONSET)
	mkdir -p $(MACOS) $(RES)
	@# Cargo names the binary arbos-desktop; the bundle calls it Arbos
	@# (CFBundleExecutable). The kernel rides beside it and is found there
	@# first (src/kernel.rs, arbos_bin).
	cp target/release/arbos-desktop $(MACOS)/Arbos
	cp $(KERNEL) $(MACOS)/arbos-kernel
	sed -e 's/@VERSION@/$(VERSION)/g' -e 's/@BUILD@/$(BUILD)/g' bundle/Info.plist > $(APP)/Contents/Info.plist
	plutil -lint $(APP)/Contents/Info.plist >/dev/null
	$(MAKE) --no-print-directory icon
	cp $(OUT)/AppIcon.icns $(RES)/AppIcon.icns
	@# The 256 png beside the .icns is the app's own: settings paints the
	@# mark itself, and nothing in the app can read an .icns.
	sips -s format png -Z 256 $(LOGO) --out $(RES)/icon.png >/dev/null
	@# Inside-out: the kernel first, then the bundle around it.
	$(call CODESIGN,$(BUNDLE_ID).kernel) $(MACOS)/arbos-kernel
	$(call CODESIGN,$(BUNDLE_ID)) $(APP)
	codesign --verify --strict --deep $(APP)
	@echo "built $(APP) $(VERSION) ($(BUILD), $(SHA)) signed by '$(SIGN)'"

# The icon, from the one logo: the trunk mark on the HIG squircle at 1024,
# then every size iconutil wants. `swift` runs the script; no toolchain
# beyond Xcode's command line tools.
icon:
	mkdir -p $(OUT) $(ICONSET)
	swift bundle/artwork.swift icon $(LOGO) $(ICON1024)
	@for s in $(SIZES); do \
		sips -s format png -Z $$s $(ICON1024) --out $(ICONSET)/icon_$${s}x$${s}.png >/dev/null; \
		sips -s format png -Z $$((s * 2)) $(ICON1024) --out $(ICONSET)/icon_$${s}x$${s}@2x.png >/dev/null; \
	done
	iconutil -c icns $(ICONSET) -o $(OUT)/AppIcon.icns
	rm -rf $(ICONSET)

dmg: bundle
	rm -rf $(DMGSTAGE) $(DMGRW) $(DMG)
	mkdir -p $(DMGSTAGE)/.background
	cp -R $(APP) $(DMGSTAGE)/
	@# Drag-to-install is this symlink and nothing else.
	ln -s /Applications $(DMGSTAGE)/Applications
	swift bundle/artwork.swift dmg $(LOGO) $(DMGSTAGE)/.background/background.png
	cp $(OUT)/AppIcon.icns $(DMGSTAGE)/.VolumeIcon.icns
	hdiutil create -volname "$(DMGVOL)" -srcfolder $(DMGSTAGE) -ov -quiet -format UDRW -fs HFS+ $(DMGRW)
	@# `.VolumeIcon.icns` is inert until the volume root carries the
	@# custom-icon bit, and Finder's window layout lives in the volume's
	@# .DS_Store; both need the image mounted read-write, which is why it is
	@# built that way and compressed afterwards. Detaching is unconditional:
	@# a failure here must not leave the volume mounted.
	@# Mounted under /Volumes, browsable, so Finder can be told about it;
	@# `-nobrowse` hides it from Finder and the layout script with it.
	@hdiutil attach $(DMGRW) $(if $(filter 1,$(DMG_LAYOUT)),,-nobrowse) -quiet -mountpoint "$(DMGMNT)" && { \
		if [ "$(DMG_LAYOUT)" = "1" ]; then \
			osascript bundle/dmg-layout.applescript "$(DMGVOL)" "$(DMGMNT)" || echo "dmg: Finder layout skipped (no Automation permission?)"; \
		fi; \
		xattr -wx com.apple.FinderInfo $(CUSTOMICON) "$(DMGMNT)"; \
		sync; hdiutil detach "$(DMGMNT)" -quiet || { sleep 2; hdiutil detach "$(DMGMNT)" -force -quiet; }; }
	@# lzfse over zlib: same second, a tenth off the download. Read since
	@# 10.11, well under the bundle's own floor.
	hdiutil convert $(DMGRW) -format ULFO -o $(DMG) -quiet
	rm -rf $(DMGRW) $(DMGSTAGE)
	@# The image is a container rather than code: it takes the signature but
	@# not the runtime flag.
	codesign --force --timestamp --sign "$(SIGN)" $(DMG)
	@echo "built $(DMG)"

# Send the image to Apple and staple the ticket to it. The app inside is
# covered by the same submission (notarytool scans the whole image); it
# verifies against Apple on first launch when dragged out, which costs a
# stapled app one round trip and nothing else.
notarize:
	@test -f $(DMG) || { echo "no $(DMG): run make dmg first"; exit 1; }
	@if [ -n "$(NOTARY_PROFILE)" ]; then \
		xcrun notarytool submit $(DMG) --keychain-profile "$(NOTARY_PROFILE)" --wait; \
	elif [ -n "$(APPLE_ID)" ] && [ -n "$(APPLE_TEAM_ID)" ] && [ -n "$(APPLE_APP_PASSWORD)" ]; then \
		xcrun notarytool submit $(DMG) --apple-id "$(APPLE_ID)" --team-id "$(APPLE_TEAM_ID)" --password "$(APPLE_APP_PASSWORD)" --wait; \
	else \
		echo "notarize: set NOTARY_PROFILE, or APPLE_ID + APPLE_TEAM_ID + APPLE_APP_PASSWORD"; exit 1; \
	fi
	xcrun stapler staple $(DMG)
	xcrun stapler validate $(DMG)

release:
	@case "$(SIGN)" in "-"|"") echo 'release: SIGN must name a "Developer ID Application" certificate'; exit 1;; esac
	@security find-identity -v -p codesigning | grep -q "$(SIGN)" || { echo "release: '$(SIGN)' is not in the keychain"; exit 1; }
	$(MAKE) --no-print-directory dmg SIGN="$(SIGN)"
	$(MAKE) --no-print-directory notarize
	$(MAKE) --no-print-directory verify
	@echo "release ready: $(DMG)"

# What Gatekeeper will do. Ad-hoc builds are "rejected" here by design; a
# Developer ID build that is notarized says "accepted, source=Notarized
# Developer ID".
verify:
	codesign -dvv --strict --deep $(APP) 2>&1 | grep -E "Identifier|Authority|TeamIdentifier|Timestamp|flags" || true
	codesign -d --entitlements - --xml $(APP) 2>/dev/null | grep -o "audio-input" || echo "warning: no microphone entitlement"
	-spctl -a -vv -t exec $(APP)
	@if [ -f $(DMG) ]; then spctl -a -vv -t open --context context:primary-signature $(DMG) || true; xcrun stapler validate $(DMG) || true; fi

open: bundle
	open $(APP)

clean:
	rm -rf $(OUT)

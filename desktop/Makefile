# Arbos.app, the disk image, and the signed + notarized release.
#
#   make bundle            Arbos.app, ad-hoc signed (opens on this Mac)
#   make dmg               Arbos-<version>-<arch>.dmg around it
#   make zip               the payload the in-app updater installs
#   make release           the same with a Developer ID, notarized, stapled
#   make verify            what Gatekeeper will say about the app and image
#   make ident             "<version> <build> <arch>", for CI to name files by
#   make unregister-strays every other bundle claiming this app's identifier
#
# Signing is inside-out: the kernel first, then the app around it, then the
# image. Every binary gets the hardened runtime and a secure timestamp — the
# two things notarization refuses without — plus bundle/entitlements.plist.
#
# Release needs two things only Apple can hand out, both kept out of the
# tree. SIGN="Developer ID Application: <name> (<team>)" names the certificate
# in the login keychain — and it has to be that kind: Apple's notary service
# refuses an "Apple Development" certificate, which can sign and can never be
# notarized.
#
# Notarization then takes any one of three credentials, tried in this order:
#
#   NOTARY_KEY + NOTARY_KEY_ID + NOTARY_ISSUER_ID
#       An App Store Connect API key — the `.p8` file, the ten-character key
#       id, and the team's issuer id. Preferred: it is a key made for this,
#       revocable on its own, and it does not involve an Apple ID password.
#   NOTARY_PROFILE
#       A keychain profile, made once with `xcrun notarytool store-credentials`.
#       Convenient on a Mac somebody is sitting at.
#   APPLE_ID + APPLE_TEAM_ID + APPLE_APP_PASSWORD
#       An app-specific password from appleid.apple.com.
#
# `.env.release` beside this file may export any of them.

VERSION  := $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
# Build number: commits on the branch, so two builds of one version sort.
# It is also the half of the version the update feed compares on — `0.2.0`
# does not move for weeks, and the dev channel publishes a build per commit.
# Overridable on the command line (`make zip BUILD=1877`) so CI names the
# payload and stamps the bundle from one number it worked out once.
BUILD    := $(shell git rev-list --count HEAD 2>/dev/null || echo 0)
SHA      := $(shell git rev-parse --short=12 HEAD 2>/dev/null || echo unknown)
ARCH     := $(shell uname -m)
LOGO     := bundle/icon.png
# The identifier the signature is designated by — read from the plist so the
# two cannot drift.
BUNDLE_ID := $(shell plutil -extract CFBundleIdentifier raw -o - bundle/Info.plist)
# `.noindex` is the suffix Spotlight is told to skip, and everything this
# Makefile builds goes under it.
#
# Not a tidiness choice. Every copy of the app carries the same
# CFBundleIdentifier, and Launch Services resolves an identifier to whichever
# registered copy it likes — so an indexed build output is a candidate for
# "open life.arbos.desktop" on equal footing with the installed app. On
# Jacob's Mac five bundles shared the identifier, all of them build outputs
# and disk-image staging folders, and ⌘Space "arbos" opened a stale scratch
# build instead of /Applications/Arbos.app. A build output that is never
# indexed can never win that race.
#
# `make unregister-strays` clears the ones already registered.
OUT      := target/bundle.noindex
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
# The update payload: the bundle itself, archived. A disk image is what a
# person drags from; a zip is what a program unpacks, and the version and
# build are in the name so one download directory can hold several.
ZIP      := $(OUT)/Arbos-$(VERSION)-$(BUILD)-macos-$(ARCH).zip
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

# Launch Services' own register. Not on PATH, and not somewhere Apple
# promises it will stay, so every use of it is guarded.
LSREGISTER := /System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister

.PHONY: build bundle icon dmg zip notarize notarize-zip release verify open clean ident unregister-strays

# What this tree would build, for a script that has to name the file before
# the file exists. One line, three words: version, build, arch.
ident:
	@echo "$(VERSION) $(BUILD) $(ARCH)"

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
		test -x $(LSREGISTER) && $(LSREGISTER) -u "$(DMGMNT)/Arbos.app" || true; \
		sync; hdiutil detach "$(DMGMNT)" -quiet || { sleep 2; hdiutil detach "$(DMGMNT)" -force -quiet; }; }
	@# lzfse over zlib: same second, a tenth off the download. Read since
	@# 10.11, well under the bundle's own floor.
	hdiutil convert $(DMGRW) -format ULFO -o $(DMG) -quiet
	rm -rf $(DMGRW) $(DMGSTAGE)
	@# The image is a container rather than code: it takes the signature but
	@# not the runtime flag.
	codesign --force --timestamp --sign "$(SIGN)" $(DMG)
	@echo "built $(DMG)"

# The payload the in-app updater fetches and installs. `ditto` and not `zip`:
# it is the only archiver that keeps a code signature valid across the round
# trip, along with the symlinks a framework is built from and the extended
# attributes a bundle carries. Unzipping it with `ditto -x -k` gives back the
# app that went in, byte for byte, signature and all.
#
# Nothing is done to it here about trust. What makes this file safe to install
# is the Ed25519 signature the publish step puts in the update feed over these
# exact bytes (crates/arbos-update), which needs nothing from Apple and so
# works today. Notarization is the separate promise below.
zip: bundle
	rm -f $(ZIP)
	ditto -c -k --sequesterRsrc --keepParent $(APP) $(ZIP)
	@echo "built $(ZIP)"

# Send a file to Apple and staple the ticket to it. Defaults to the disk
# image; `make notarize NOTARIZE_FILE=...` sends something else.
#
# For the image, the app inside is covered by the same submission (notarytool
# scans the whole image); it verifies against Apple on first launch when
# dragged out, which costs a stapled app one round trip and nothing else.
NOTARIZE_FILE ?= $(DMG)
notarize:
	@test -f $(NOTARIZE_FILE) || { echo "no $(NOTARIZE_FILE): build it first"; exit 1; }
	@# `--wait` makes a refusal this command's failure, which is what stops a
	@# rejected payload being published: everything downstream is a
	@# prerequisite of this rule.
	@if [ -n "$(NOTARY_KEY)" ] && [ -n "$(NOTARY_KEY_ID)" ] && [ -n "$(NOTARY_ISSUER_ID)" ]; then \
		xcrun notarytool submit $(NOTARIZE_FILE) --key "$(NOTARY_KEY)" --key-id "$(NOTARY_KEY_ID)" --issuer "$(NOTARY_ISSUER_ID)" --wait; \
	elif [ -n "$(NOTARY_PROFILE)" ]; then \
		xcrun notarytool submit $(NOTARIZE_FILE) --keychain-profile "$(NOTARY_PROFILE)" --wait; \
	elif [ -n "$(APPLE_ID)" ] && [ -n "$(APPLE_TEAM_ID)" ] && [ -n "$(APPLE_APP_PASSWORD)" ]; then \
		xcrun notarytool submit $(NOTARIZE_FILE) --apple-id "$(APPLE_ID)" --team-id "$(APPLE_TEAM_ID)" --password "$(APPLE_APP_PASSWORD)" --wait; \
	else \
		echo "notarize: set NOTARY_KEY + NOTARY_KEY_ID + NOTARY_ISSUER_ID, or NOTARY_PROFILE, or APPLE_ID + APPLE_TEAM_ID + APPLE_APP_PASSWORD"; exit 1; \
	fi
	@# A zip is a container Apple will scan but cannot hold a ticket, so the
	@# ticket goes on what came out of it and the caller re-archives.
	@case "$(NOTARIZE_FILE)" in \
		*.zip) xcrun stapler staple $(APP); xcrun stapler validate $(APP);; \
		*) xcrun stapler staple $(NOTARIZE_FILE); xcrun stapler validate $(NOTARIZE_FILE);; \
	esac

# The update payload with Apple's ticket in it, for the day the Developer ID
# certificate exists. The app is stapled first and the zip made again around
# it, so a Mac with no network still sees a notarized app — which is the whole
# point of stapling. The feed's own signature is unaffected: it is taken over
# whichever bytes are published, and these are simply better bytes.
notarize-zip: zip
	$(MAKE) --no-print-directory notarize NOTARIZE_FILE=$(ZIP)
	@# The last word on whether this would be rejected, asked of the thing
	@# that does the rejecting. `notarytool --wait` already fails on a
	@# refusal and `stapler validate` on a missing ticket, but neither is
	@# Gatekeeper; this is. A payload that cannot pass here must not be
	@# published, so the failure is the build's.
	@spctl -a -vv -t exec $(APP) 2>&1 | tee $(OUT)/gatekeeper.txt
	@grep -q "source=Notarized Developer ID" $(OUT)/gatekeeper.txt || { \
		echo "notarize-zip: Gatekeeper did not accept $(APP) as notarized"; \
		rm -f $(OUT)/gatekeeper.txt; exit 1; }
	@rm -f $(OUT)/gatekeeper.txt
	rm -f $(ZIP)
	ditto -c -k --sequesterRsrc --keepParent $(APP) $(ZIP)
	@echo "notarized $(ZIP)"

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

# Every bundle on this Mac claiming this app's identifier, except the one in
# /Applications, taken out of Launch Services' register.
#
# This is the repair for a Mac that already has strays — old build outputs,
# a disk image that was mounted once, a copy dragged somewhere to try it.
# They all carry the same CFBundleIdentifier, so any of them can answer for
# it, and ⌘Space opens whichever the register hands back. Nothing is deleted:
# a build output goes on being a build output, it just stops claiming to be
# the installed app.
#
# Run it once. `OUT` being under `.noindex` is what stops new ones appearing.
unregister-strays:
	@test -x $(LSREGISTER) || { echo "no lsregister on this machine"; exit 0; }
	@$(LSREGISTER) -dump \
		| awk '/^[[:space:]]*path:/ { p = $$2; for (i = 3; i <= NF; i++) p = p " " $$i } \
		       /identifier:[[:space:]]*$(BUNDLE_ID)([^a-zA-Z0-9.]|$$)/ { if (p != "") print p }' \
		| sort -u \
		| grep -v '^/Applications/Arbos\.app$$' \
		| while IFS= read -r stray; do \
			echo "unregistering $$stray"; \
			$(LSREGISTER) -u "$$stray" || true; \
		done
	@echo "done — /Applications/Arbos.app is the only $(BUNDLE_ID) left registered"

clean:
	@# Take the build output out of the register on the way out, so a bundle
	@# that was opened from here cannot answer for the identifier after the
	@# directory it lived in is gone.
	@test -x $(LSREGISTER) && test -d $(APP) && $(LSREGISTER) -u $(APP) || true
	rm -rf $(OUT)

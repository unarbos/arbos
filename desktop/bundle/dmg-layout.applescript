-- Lays out the mounted disk image the way the backdrop expects: a 660 × 400
-- window, icons at 128 px, Arbos on the left spot and the Applications link
-- on the right, the arrow between them painted by .background/background.png.
-- Finder writes the result into the volume's .DS_Store, which the Makefile
-- then freezes into the compressed image.
--
--   osascript bundle/dmg-layout.applescript "Arbos 0.2.0" /path/to/mountpoint
--
-- Running this needs Automation permission for Finder from whatever calls
-- it (a terminal, CI's shell). The Makefile treats failure as cosmetic.

on run argv
	set volumeName to item 1 of argv
	set mountPath to item 2 of argv
	tell application "Finder"
		tell disk volumeName
			open
			set current view of container window to icon view
			set toolbar visible of container window to false
			set statusbar visible of container window to false
			set the bounds of container window to {200, 120, 860, 520}
			set viewOptions to the icon view options of container window
			set arrangement of viewOptions to not arranged
			set icon size of viewOptions to 128
			set text size of viewOptions to 13
			set background picture of viewOptions to file ".background:background.png"
			set position of item "Arbos.app" of container window to {165, 190}
			set position of item "Applications" of container window to {495, 190}
			close
			open
			update without registering applications
			delay 1
			close
		end tell
	end tell
end run

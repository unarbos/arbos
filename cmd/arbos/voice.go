package main

import (
	"context"
	"crypto/sha256"
	_ "embed"
	"encoding/hex"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"time"
)

// voiceDictateSrc is the on-device dictation helper, compiled on first use.
//
//go:embed voice_dictate.swift
var voiceDictateSrc string

// voiceInfoPlist is the dictation app's Info.plist. The usage-description keys
// are what let macOS show the microphone / speech-recognition prompts; the app
// is launched via `open` so it is its own TCC-responsible process and these
// strings (not the launching terminal's) are read.
const voiceInfoPlist = `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key>
  <string>dictate</string>
  <key>CFBundleIdentifier</key>
  <string>com.unarbos.arbos.voice</string>
  <key>CFBundleName</key>
  <string>arbos voice</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleVersion</key>
  <string>1</string>
  <key>CFBundleShortVersionString</key>
  <string>1.0</string>
  <key>LSUIElement</key>
  <true/>
  <key>NSMicrophoneUsageDescription</key>
  <string>arbos transcribes your speech into the chat composer.</string>
  <key>NSSpeechRecognitionUsageDescription</key>
  <string>arbos transcribes your speech into the chat composer.</string>
</dict>
</plist>
`

// voiceDonePoll is how often Stop checks for the helper's completion marker.
const voiceDonePoll = 100 * time.Millisecond

// voiceStopTimeout bounds how long Stop waits for the transcript after asking
// the helper to end — generous, since a first run may pause on the macOS
// permission prompt.
const voiceStopTimeout = 30 * time.Second

// hostVoice captures speech from the local machine's microphone and
// transcribes it on-device (Apple's Speech framework, via a tiny app bundle
// launched with `open`). One recording at a time — there is one mic. It
// satisfies gateway.VoiceRecorder.
type hostVoice struct {
	mu     sync.Mutex
	recDir string // per-recording control dir; "" when idle
	out    string // transcript path the helper writes
	stop   string // stop sentinel the helper watches
}

// voiceHelperApp compiles + signs the dictation app bundle once (keyed by a
// hash of its source) under the user cache dir, returning the .app path. It
// rebuilds only when the source or plist changes.
func voiceHelperApp() (string, error) {
	sum := sha256.Sum256([]byte(voiceDictateSrc + voiceInfoPlist))
	tag := hex.EncodeToString(sum[:8])
	cacheRoot, err := os.UserCacheDir()
	if err != nil {
		cacheRoot = os.TempDir()
	}
	dir := filepath.Join(cacheRoot, "arbos", "voice")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return "", err
	}
	app := filepath.Join(dir, "arbos-dictate-"+tag+".app")
	bin := filepath.Join(app, "Contents", "MacOS", "dictate")
	if _, err := os.Stat(bin); err == nil {
		return app, nil
	}

	// Build into a temp app, then rename into place so a half-built bundle is
	// never observed.
	tmp := app + ".building"
	_ = os.RemoveAll(tmp)
	macos := filepath.Join(tmp, "Contents", "MacOS")
	if err := os.MkdirAll(macos, 0o755); err != nil {
		return "", err
	}
	src := filepath.Join(tmp, "dictate.swift")
	if err := os.WriteFile(src, []byte(voiceDictateSrc), 0o644); err != nil {
		return "", err
	}
	plist := filepath.Join(tmp, "Contents", "Info.plist")
	if err := os.WriteFile(plist, []byte(voiceInfoPlist), 0o644); err != nil {
		return "", err
	}

	ctx, cancel := context.WithTimeout(context.Background(), 120*time.Second)
	defer cancel()
	if out, err := exec.CommandContext(ctx, "swiftc", "-O", "-o",
		filepath.Join(macos, "dictate"), src).CombinedOutput(); err != nil {
		return "", fmt.Errorf("compile voice helper: %w: %s", err, strings.TrimSpace(string(out)))
	}
	_ = os.Remove(src)
	// Ad-hoc sign so TCC binds the permission grant to a stable identity.
	if out, err := exec.CommandContext(ctx, "codesign", "--force", "--sign", "-", tmp).CombinedOutput(); err != nil {
		return "", fmt.Errorf("sign voice helper: %w: %s", err, strings.TrimSpace(string(out)))
	}
	if err := os.Rename(tmp, app); err != nil {
		return "", err
	}
	return app, nil
}

// Start begins capturing from the host microphone. The recording outlives the
// request that triggers it: the helper is an independent `open`-launched app,
// coordinated through files, and Stop ends it.
func (v *hostVoice) Start(_ context.Context) error {
	v.mu.Lock()
	defer v.mu.Unlock()
	if v.recDir != "" {
		return errors.New("a recording is already in progress")
	}
	app, err := voiceHelperApp()
	if err != nil {
		return err
	}
	recDir, err := os.MkdirTemp("", "arbos-voice-")
	if err != nil {
		return err
	}
	out := filepath.Join(recDir, "transcript.txt")
	stop := filepath.Join(recDir, "stop")
	// -n forces a fresh instance, -g keeps it from stealing focus.
	cmd := exec.Command("open", "-n", "-g", app, "--args", "--out", out, "--stop", stop)
	if combined, err := cmd.CombinedOutput(); err != nil {
		_ = os.RemoveAll(recDir)
		return fmt.Errorf("launch voice helper: %w: %s", err, strings.TrimSpace(string(combined)))
	}
	v.recDir, v.out, v.stop = recDir, out, stop
	return nil
}

// Stop ends the recording and returns the transcribed text: it drops the stop
// sentinel the helper watches, waits for the completion marker, then reads the
// transcript (or the error the helper recorded).
func (v *hostVoice) Stop(ctx context.Context) (string, error) {
	v.mu.Lock()
	recDir, out, stop := v.recDir, v.out, v.stop
	v.recDir, v.out, v.stop = "", "", ""
	v.mu.Unlock()

	if recDir == "" {
		return "", errors.New("no recording in progress")
	}
	defer func() { _ = os.RemoveAll(recDir) }()

	if err := os.WriteFile(stop, nil, 0o644); err != nil {
		return "", fmt.Errorf("signal voice helper: %w", err)
	}

	done := out + ".done"
	deadline := time.After(voiceStopTimeout)
	tick := time.NewTicker(voiceDonePoll)
	defer tick.Stop()
	for {
		if _, err := os.Stat(done); err == nil {
			break
		}
		select {
		case <-ctx.Done():
			return "", ctx.Err()
		case <-deadline:
			return "", errors.New("voice transcription timed out")
		case <-tick.C:
		}
	}

	if msg, err := os.ReadFile(out + ".err"); err == nil {
		return "", errors.New(strings.TrimSpace(string(msg)))
	}
	text, err := os.ReadFile(out)
	if err != nil {
		return "", fmt.Errorf("read transcript: %w", err)
	}
	return strings.TrimSpace(string(text)), nil
}

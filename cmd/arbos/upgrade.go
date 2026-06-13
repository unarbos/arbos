package main

import (
	"archive/tar"
	"bufio"
	"bytes"
	"compress/gzip"
	"crypto/sha256"
	"errors"
	"flag"
	"fmt"
	"io"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"time"
)

// arbosModule is the module this binary is built from — the needle `upgrade`
// looks for to decide source mode, and the path it `go install`s in release
// mode.
const arbosModule = "github.com/unarbos/arbos"

// runUpgrade is `arbos upgrade`: produce a newer binary at the running
// server's executable path and let the server's self-restart watcher
// (engine.WatchRestart) hot-swap it at the next idle turn boundary. This is
// the single canonical update verb — for a human in a terminal, and for the
// agent told to "update yourself" (its tool shells inherit ARBOS_EXE from the
// server, so the right file is replaced no matter what PATH says).
//
// Two ways the new binary is produced, detected not flagged:
//
//   - source mode: the working directory is inside an arbos source checkout
//     (a go.mod whose module is github.com/unarbos/arbos) — build the
//     checkout. This is the self-editing path: edit, `arbos upgrade`, done.
//   - release mode: no checkout — download the prebuilt binary for this
//     platform from the latest GitHub release (checksum-verified), falling
//     back to `go install <module>@latest` when no release asset exists.
//     Downloading instead of compiling matters on small nodes: a Go build
//     wants gigabytes of RAM and can get the serving process OOM-killed.
//
// Both paths stage to <target>.new and rename: the swap is atomic, and a
// failed build or download never touches the binary that is serving.
func runUpgrade(args []string) error {
	fs := flag.NewFlagSet("upgrade", flag.ContinueOnError)
	fs.SetOutput(io.Discard)
	var target string
	fs.StringVar(&target, "to", "", "")
	if err := fs.Parse(normalizeLongFlags(args)); err != nil {
		return fmt.Errorf("upgrade: %w", err)
	}
	if len(fs.Args()) > 0 {
		return fmt.Errorf("upgrade: unexpected argument %q", fs.Args()[0])
	}

	if target == "" {
		target = os.Getenv("ARBOS_EXE")
	}
	if target == "" {
		exe, err := os.Executable()
		if err != nil {
			return fmt.Errorf("upgrade: cannot resolve own executable: %w", err)
		}
		target = exe
	}
	target, err := filepath.EvalSymlinks(target)
	if err != nil {
		return fmt.Errorf("upgrade: target %s: %w", target, err)
	}

	cwd, err := os.Getwd()
	if err != nil {
		return err
	}

	if root := findSourceRoot(cwd); root != "" {
		return upgradeFromSource(root, target)
	}
	return upgradeFromRelease(target)
}

// findSourceRoot walks up from dir looking for the arbos checkout: the first
// go.mod whose module line names arbosModule. Returns "" when dir is not
// inside one (release mode).
func findSourceRoot(dir string) string {
	for {
		if mod, err := os.Open(filepath.Join(dir, "go.mod")); err == nil {
			s := bufio.NewScanner(mod)
			for s.Scan() {
				line := strings.TrimSpace(s.Text())
				if module, ok := strings.CutPrefix(line, "module "); ok {
					_ = mod.Close()
					if strings.TrimSpace(module) == arbosModule {
						return dir
					}
					return "" // inside some other module: not our checkout
				}
			}
			_ = mod.Close()
		}
		parent := filepath.Dir(dir)
		if parent == dir {
			return ""
		}
		dir = parent
	}
}

// upgradeFromSource builds the checkout at root and atomically installs the
// result at target. Tool schemas are regenerated first, same as the dev loop,
// so a self-edit that touched tool specs ships complete.
func upgradeFromSource(root, target string) error {
	fmt.Printf("arbos upgrade: source mode — building %s\n", root)
	if err := runIn(root, "go", "generate", "./internal/tool/coding"); err != nil {
		return fmt.Errorf("upgrade: generate: %w", err)
	}
	staged := target + ".new"
	if err := runIn(root, "go", "build", "-o", staged, "./cmd/arbos"); err != nil {
		_ = os.Remove(staged)
		return fmt.Errorf("upgrade: build failed, %s untouched: %w", target, err)
	}
	if err := os.Rename(staged, target); err != nil {
		_ = os.Remove(staged)
		return fmt.Errorf("upgrade: install: %w", err)
	}
	reportSwap(target)
	return nil
}

// releaseDownloadBase is the stable URL the release workflow
// (.github/workflows/release.yml) publishes versionless assets under, so
// "latest" needs no API call or token. ARBOS_RELEASE_BASE overrides it
// (forks, local testing) — same knob install.sh honors.
const releaseDownloadBase = "https://github.com/unarbos/arbos/releases/latest/download"

func releaseBase() string {
	if v := os.Getenv("ARBOS_RELEASE_BASE"); v != "" {
		return strings.TrimRight(v, "/")
	}
	return releaseDownloadBase
}

// upgradeFromRelease puts the latest published arbos at target: the prebuilt
// binary for this platform when a release asset exists (a ~20 MB download —
// no Go toolchain, no compile, no build-sized RAM spike next to the serving
// process), else `go install` as the fallback for platforms without assets
// or before the first release.
func upgradeFromRelease(target string) error {
	before := buildVersion()
	fmt.Printf("arbos upgrade: release mode — downloading latest %s/%s build\n", runtime.GOOS, runtime.GOARCH)
	if err := downloadRelease(target); err != nil {
		fmt.Printf("arbos upgrade: no prebuilt release (%v) — building with go install instead\n", err)
		if err := upgradeFromGoInstall(target); err != nil {
			return err
		}
	}
	after := strings.TrimSpace(versionOf(target))
	if after != "" && after == before {
		fmt.Printf("arbos upgrade: already up to date (%s)\n", after)
		return nil
	}
	if after != "" {
		fmt.Printf("arbos upgrade: %s → %s\n", before, after)
	}
	reportSwap(target)
	return nil
}

// downloadRelease fetches this platform's tarball from the latest GitHub
// release, verifies it against the release's checksums.txt, stages the
// binary at <target>.new, proves it runs (--version), and renames it over
// target. Any error leaves target untouched.
func downloadRelease(target string) error {
	asset := fmt.Sprintf("arbos_%s_%s.tar.gz", runtime.GOOS, runtime.GOARCH)
	client := &http.Client{Timeout: 5 * time.Minute}

	sums, err := fetchBytes(client, releaseBase()+"/checksums.txt")
	if err != nil {
		return err
	}
	wantSum := ""
	for line := range strings.Lines(string(sums)) {
		if f := strings.Fields(line); len(f) == 2 && f[1] == asset {
			wantSum = f[0]
		}
	}
	if wantSum == "" {
		return fmt.Errorf("no %s in release checksums", asset)
	}

	tarball, err := fetchBytes(client, releaseBase()+"/"+asset)
	if err != nil {
		return err
	}
	if got := fmt.Sprintf("%x", sha256.Sum256(tarball)); got != wantSum {
		return fmt.Errorf("%s checksum mismatch: got %s, want %s", asset, got, wantSum)
	}

	bin, err := extractBinary(tarball, "arbos")
	if err != nil {
		return fmt.Errorf("%s: %w", asset, err)
	}
	// Identical bytes: nothing to swap. Skipping the rename matters under a
	// serving arbos — a fresh inode would trigger a re-exec into the same
	// build for nothing.
	if cur, err := os.ReadFile(target); err == nil && bytes.Equal(cur, bin) {
		return nil
	}
	staged := target + ".new"
	if err := os.WriteFile(staged, bin, 0o755); err != nil {
		return err
	}
	// Preflight: a binary that cannot even print its version (truncated,
	// wrong arch) must never be renamed over the one that is serving.
	if v := strings.TrimSpace(versionOf(staged)); v == "" {
		_ = os.Remove(staged)
		return fmt.Errorf("downloaded binary failed to run")
	}
	if err := os.Rename(staged, target); err != nil {
		_ = os.Remove(staged)
		return err
	}
	return nil
}

// fetchBytes GETs url fully, following GitHub's release-asset redirects.
func fetchBytes(client *http.Client, url string) ([]byte, error) {
	resp, err := client.Get(url)
	if err != nil {
		return nil, err
	}
	defer func() { _ = resp.Body.Close() }()
	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("GET %s: %s", url, resp.Status)
	}
	return io.ReadAll(resp.Body)
}

// extractBinary returns the named file's bytes from a gzipped tarball.
func extractBinary(tarball []byte, name string) ([]byte, error) {
	gz, err := gzip.NewReader(bytes.NewReader(tarball))
	if err != nil {
		return nil, err
	}
	tr := tar.NewReader(gz)
	for {
		hdr, err := tr.Next()
		if err != nil {
			return nil, fmt.Errorf("no %q in archive: %w", name, err)
		}
		if filepath.Base(hdr.Name) == name && hdr.Typeflag == tar.TypeReg {
			return io.ReadAll(tr)
		}
	}
}

// upgradeFromGoInstall is the no-release-asset fallback: `go install
// <module>@latest`, then copy the installed binary over the target when they
// differ. `go install` is itself atomic for the Go bin path, and the copy
// stages to <target>.new for the same guarantee elsewhere.
func upgradeFromGoInstall(target string) error {
	if err := runIn("", "go", "install", arbosModule+"/cmd/arbos@latest"); err != nil {
		return fmt.Errorf("upgrade: go install: %w", err)
	}
	installed, err := goInstalledBinary()
	if err != nil {
		return err
	}
	if resolved, err := filepath.EvalSymlinks(installed); err == nil && resolved == target {
		return nil // go install already wrote the target in place.
	}
	if err := copyFile(installed, target); err != nil {
		return fmt.Errorf("upgrade: install to %s: %w", target, err)
	}
	return nil
}

// goInstalledBinary returns the path `go install` writes the arbos binary to:
// GOBIN when set, else GOPATH/bin.
func goInstalledBinary() (string, error) {
	out, err := exec.Command("go", "env", "GOBIN", "GOPATH").Output()
	if err != nil {
		return "", fmt.Errorf("upgrade: go env: %w", err)
	}
	// One line per key, in order; GOBIN is usually empty (an empty first
	// line), so split before trimming anything.
	lines := strings.Split(string(out), "\n")
	if len(lines) < 2 {
		return "", errors.New("upgrade: unexpected go env output")
	}
	gobin, gopath := strings.TrimSpace(lines[0]), strings.TrimSpace(lines[1])
	dir := gobin
	if dir == "" {
		dir = filepath.Join(gopath, "bin")
	}
	return filepath.Join(dir, "arbos"), nil
}

// copyFile stages src's bytes at dst.new and renames over dst, preserving an
// executable mode. Staging in dst's directory keeps the rename atomic (same
// filesystem).
func copyFile(src, dst string) error {
	in, err := os.Open(src)
	if err != nil {
		return err
	}
	defer func() { _ = in.Close() }()
	staged := dst + ".new"
	out, err := os.OpenFile(staged, os.O_WRONLY|os.O_CREATE|os.O_TRUNC, 0o755)
	if err != nil {
		return err
	}
	if _, err := io.Copy(out, in); err != nil {
		_ = out.Close()
		_ = os.Remove(staged)
		return err
	}
	if err := out.Close(); err != nil {
		_ = os.Remove(staged)
		return err
	}
	if err := os.Rename(staged, dst); err != nil {
		_ = os.Remove(staged)
		return err
	}
	return nil
}

// versionOf asks a binary for its version; "" when it cannot say.
func versionOf(bin string) string {
	out, err := exec.Command(bin, "--version").Output()
	if err != nil {
		return ""
	}
	return string(out)
}

// reportSwap tells the operator what happens next. When run inside a serving
// arbos (ARBOS_EXE set), the swap is automatic; from a bare terminal with no
// server running, the next launch simply uses the new binary.
func reportSwap(target string) {
	fmt.Printf("arbos upgrade: %s replaced\n", target)
	if os.Getenv("ARBOS_EXE") != "" {
		fmt.Println("arbos upgrade: the running server will re-exec the new binary at its next idle moment — no restart needed")
	}
}

// runIn streams a command's output through, in dir when non-empty.
func runIn(dir, name string, args ...string) error {
	cmd := exec.Command(name, args...)
	cmd.Dir = dir
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr
	return cmd.Run()
}

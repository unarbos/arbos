package forest

import (
	"crypto/ed25519"
	"crypto/rand"
	"crypto/sha256"
	"fmt"
	"os"
	"path/filepath"
)

// agentDigest is the stable identity of an agent: its device public key
// composed with its per-directory agent public key (ADR-0035), hashed. The
// full digest is the lease owner key; nameFromHash projects it to a readable
// name. An empty agentPub names the device alone.
func agentDigest(devicePub, agentPub []byte) []byte {
	sum := sha256.New()
	sum.Write(devicePub)
	sum.Write(agentPub)
	return sum.Sum(nil)
}

// agentKeyName is the per-directory key file under a workspace's .arbos dir.
const agentKeyName = "agent.key"

// LoadOrCreateDeviceKey returns the device's ed25519 key, generating one on
// first run. The keypair is the device (ADR-0033): the only durable local
// secret, stored as the 32-byte seed at path (0600, dir 0700). The account
// subsystem (ADR-0033 P1) will lift this same file; the forest client is its
// first consumer.
func LoadOrCreateDeviceKey(path string) (ed25519.PrivateKey, error) {
	return loadOrCreateSeed(path)
}

// LoadOrCreateAgentKey returns the per-directory agent key, generating one on
// first run at <dir>/.arbos/agent.key. Unlike the device key it is not a
// secret (ADR-0035): its public half is a binding input that, composed with
// the device key, derives this agent's stable URL — same directory on the same
// machine yields the same URL, with no login. It travels with the directory;
// the secret that gates the URL stays the device key, on the box.
func LoadOrCreateAgentKey(dir string) (ed25519.PrivateKey, error) {
	return loadOrCreateSeed(filepath.Join(dir, ".arbos", agentKeyName))
}

// loadOrCreateSeed reads (or, on first run, generates) a 32-byte ed25519 seed
// at path, written 0600 in a 0700 dir.
func loadOrCreateSeed(path string) (ed25519.PrivateKey, error) {
	if seed, err := os.ReadFile(path); err == nil {
		if len(seed) != ed25519.SeedSize {
			return nil, fmt.Errorf("key %s: want %d-byte seed, got %d", path, ed25519.SeedSize, len(seed))
		}
		return ed25519.NewKeyFromSeed(seed), nil
	}
	seed := make([]byte, ed25519.SeedSize)
	if _, err := rand.Read(seed); err != nil {
		return nil, err
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return nil, err
	}
	if err := os.WriteFile(path, seed, 0o600); err != nil {
		return nil, err
	}
	return ed25519.NewKeyFromSeed(seed), nil
}

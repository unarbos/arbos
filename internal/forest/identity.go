package forest

import (
	"crypto/ed25519"
	"crypto/rand"
	"fmt"
	"os"
	"path/filepath"
)

// LoadOrCreateDeviceKey returns the device's ed25519 key, generating one on
// first run. The keypair is the device (ADR-0033): the only durable local
// secret, stored as the 32-byte seed at path (0600, dir 0700). The account
// subsystem (ADR-0033 P1) will lift this same file; the forest client is its
// first consumer.
func LoadOrCreateDeviceKey(path string) (ed25519.PrivateKey, error) {
	if seed, err := os.ReadFile(path); err == nil {
		if len(seed) != ed25519.SeedSize {
			return nil, fmt.Errorf("device key %s: want %d-byte seed, got %d", path, ed25519.SeedSize, len(seed))
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

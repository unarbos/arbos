package head

import (
	"encoding/hex"

	"github.com/decred/dcrd/dcrec/secp256k1/v4"
	"golang.org/x/crypto/sha3"
)

// newDepositKey mints a fresh secp256k1 keypair and returns its EVM address
// (lowercased 0x-hex) plus the 32-byte private key. Each account gets its own
// address so a deposit is attributed by where it lands — no memo/tag, no
// submitter race. The private key is what later sweeps the funds to a cold
// treasury; until then it lives in the head's store (the documented custody
// tradeoff; an offline HD seed is the hardening).
func newDepositKey() (address string, privKey []byte, err error) {
	priv, err := secp256k1.GeneratePrivateKey()
	if err != nil {
		return "", nil, err
	}
	return evmAddress(priv.PubKey()), priv.Serialize(), nil
}

// evmAddress derives the lowercased 0x EVM address from a public key:
// keccak256(uncompressed pubkey without the 0x04 prefix), last 20 bytes.
func evmAddress(pub *secp256k1.PublicKey) string {
	uncompressed := pub.SerializeUncompressed() // 65 bytes: 0x04 || X || Y
	h := sha3.NewLegacyKeccak256()
	h.Write(uncompressed[1:])
	sum := h.Sum(nil)
	return "0x" + hex.EncodeToString(sum[12:])
}

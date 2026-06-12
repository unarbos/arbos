package forest

import (
	"crypto/rand"
	"encoding/hex"
	"math/big"
)

// Lease names are assigned, never chosen: an anonymous device cannot request
// "paypal-login", which is most of the phishing defense on a shared domain
// (ADR-0034). adjective-animal keeps them speakable; the hex tail keeps the
// pool effectively collision-free without bookkeeping.

var nameAdjectives = []string{
	"amber", "bold", "brisk", "calm", "cedar", "clever", "cosmic", "crisp",
	"dusty", "eager", "early", "fuzzy", "gentle", "glad", "golden", "green",
	"happy", "hardy", "hazel", "humble", "ivory", "jolly", "keen", "kind",
	"lively", "lucky", "lunar", "mellow", "merry", "misty", "noble", "nimble",
	"ochre", "plucky", "quiet", "rapid", "rosy", "rustic", "silent", "snowy",
	"spry", "stout", "sunny", "swift", "tidy", "violet", "warm", "witty",
}

var nameAnimals = []string{
	"badger", "bison", "bobcat", "crane", "deer", "dingo", "egret", "falcon",
	"ferret", "finch", "fox", "gecko", "hare", "heron", "ibis", "jay",
	"koala", "lemur", "lynx", "macaw", "marten", "mole", "moose", "newt",
	"ocelot", "otter", "owl", "panda", "pika", "quail", "raven", "robin",
	"seal", "shrew", "skink", "sloth", "stoat", "swan", "tapir", "tern",
	"toad", "trout", "vole", "walrus", "weasel", "wombat", "wren", "yak",
}

// mintName draws a fresh adjective-animal-hex3 lease name.
func mintName() string {
	tail := make([]byte, 2)
	_, _ = rand.Read(tail)
	return pick(nameAdjectives) + "-" + pick(nameAnimals) + "-" + hex.EncodeToString(tail)[:3]
}

func pick(list []string) string {
	n, err := rand.Int(rand.Reader, big.NewInt(int64(len(list))))
	if err != nil {
		return list[0]
	}
	return list[n.Int64()]
}

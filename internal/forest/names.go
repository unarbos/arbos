package forest

import (
	"crypto/rand"
	"encoding/hex"
	"math/big"
)

// Lease names are assigned by the head and remembered, never derived from or
// chosen by the node (ADR-0035). The head mints a readable name the first time
// it sees an agent's identity (agentID) and persists the mapping, so the same
// (machine, directory) reclaims the same name across restarts with no login.
// Because the name is assigned (and retried past collisions) rather than a
// function of the keys, it is grind-proof — a node cannot search keys for a
// chosen name — and an agent is never stuck: a clashing draw is simply
// redrawn. adjective-animal stays speakable; the hex tail keeps fresh draws
// collision-free with little retry.

var nameAdjectives = []string{
	"amber", "ashen", "azure", "bold", "brave", "brisk", "bronze", "calm",
	"cedar", "clever", "cobalt", "coral", "cosmic", "crisp", "dapper", "dewy",
	"dusky", "dusty", "eager", "early", "ember", "fabled", "fancy", "feral",
	"fern", "fleet", "fond", "frosty", "fuzzy", "gentle", "ginger", "glad",
	"golden", "grand", "green", "happy", "hardy", "hazel", "hidden", "honey",
	"humble", "icy", "indigo", "ivory", "jade", "jolly", "keen", "kind",
	"lilac", "lively", "lucky", "lunar", "mauve", "mellow", "merry", "misty",
	"noble", "nimble", "ochre", "olive", "pearl", "plucky", "prim", "quiet",
	"rapid", "rosy", "royal", "rustic", "sable", "sage", "sandy", "scarlet",
	"shy", "silent", "silver", "sleek", "snowy", "solar", "spry", "stark",
	"stout", "sunny", "swift", "teal", "tidy", "umber", "velvet", "violet",
	"warm", "wily", "windy", "witty", "woven",
}

var nameAnimals = []string{
	"badger", "bat", "bison", "bobcat", "carp", "crane", "deer", "dingo",
	"dove", "drake", "eagle", "egret", "elk", "falcon", "fawn", "ferret",
	"finch", "fox", "gecko", "goose", "grebe", "grouse", "hare", "hawk",
	"heron", "ibis", "jay", "kite", "koala", "lark", "lemur", "loon",
	"lynx", "macaw", "marten", "merlin", "mink", "mole", "moose", "moth",
	"newt", "ocelot", "orca", "oriole", "osprey", "otter", "owl", "panda",
	"perch", "pika", "pike", "plover", "puma", "quail", "rail", "raven",
	"robin", "salmon", "seal", "shrew", "skink", "sloth", "snipe", "sparrow",
	"stoat", "stork", "swan", "tanager", "tapir", "tern", "toad", "trout",
	"viper", "vole", "walrus", "weasel", "wolf", "wombat", "wren", "yak",
	"zebra", "adder", "bittern", "civet", "eider", "gannet", "godwit", "linnet",
	"petrel", "puffin", "raptor", "shrike",
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

package codingspec

import "sync"

var fileMutationLocks = mutationLocks{locks: map[string]*sync.Mutex{}}

type mutationLocks struct {
	mu    sync.Mutex
	locks map[string]*sync.Mutex
}

func lockMutation(key string) func() {
	fileMutationLocks.mu.Lock()
	l, ok := fileMutationLocks.locks[key]
	if !ok {
		l = &sync.Mutex{}
		fileMutationLocks.locks[key] = l
	}
	fileMutationLocks.mu.Unlock()

	l.Lock()
	return l.Unlock
}

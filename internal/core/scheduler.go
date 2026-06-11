package core

import (
	"strconv"
	"strings"
)

// SchedulerOriginPrefix is the sessions.origin value for a scheduler wake
// belonging to chat (no plan node).
func SchedulerOriginPrefix(chat string) string {
	return OriginScheduler + ":" + chat
}

// SchedulerOrigin is the full provenance for a scheduler wake: chat plus plan
// node.
func SchedulerOrigin(chat string, nodeID int64) string {
	if nodeID == 0 {
		return SchedulerOriginPrefix(chat)
	}
	return SchedulerOriginPrefix(chat) + "#" + strconv.FormatInt(nodeID, 10)
}

// ParseSchedulerOrigin splits a scheduler provenance string into chat and node.
func ParseSchedulerOrigin(origin string) (chat string, node int64, ok bool) {
	if !strings.HasPrefix(origin, OriginScheduler+":") {
		return "", 0, false
	}
	rest := origin[len(OriginScheduler)+1:]
	chat, nodeStr, hasNode := strings.Cut(rest, "#")
	if !hasNode {
		return chat, 0, true
	}
	n, err := strconv.ParseInt(nodeStr, 10, 64)
	if err != nil {
		return chat, 0, true
	}
	return chat, n, true
}

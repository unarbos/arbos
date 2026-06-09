package core

// EstimatedImageChars is the per-image size proxy used by token estimation: an
// inline image costs far more than its text, so it counts as a flat character
// budget (matching pi's coding-agent estimator). It only needs to be monotonic
// in payload size; exact accounting comes from provider usage events.
const EstimatedImageChars = 4800

// EstimateTokens is the canonical cheap, provider-agnostic token proxy
// (~4 chars/token) for one message, used to decide WHEN to compress. It is the
// single home for the heuristic both the engine's compression trigger and pi's
// compaction policy rely on; exact token accounting comes from usage events.
func EstimateTokens(m Message) int {
	chars := len(m.Content) + len(m.Reasoning)
	for _, p := range m.Parts {
		if p.Type == BlockImage {
			chars += EstimatedImageChars
		} else {
			chars += len(p.Text)
		}
	}
	return chars / 4
}

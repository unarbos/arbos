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
		switch p.Type {
		case BlockImage:
			chars += EstimatedImageChars
		case BlockFile:
			// A document is at least as heavy as an image; the base64 payload
			// stands in as a monotonic size proxy until a usage event corrects it.
			if p.File != nil {
				chars += EstimatedImageChars + len(p.File.Data)/4
			} else {
				chars += EstimatedImageChars
			}
		default:
			chars += len(p.Text)
		}
	}
	return chars / 4
}

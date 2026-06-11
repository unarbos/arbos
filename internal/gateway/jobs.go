package gateway

import (
	"encoding/base64"
	"net/http"
	"os"
	"strconv"
)

// JobSnapshot is the gateway's view of one background job — the host maps its
// job table's entry into this shape (the same injection seam as KillJob), so
// the gateway stays ignorant of how jobs are stored.
type JobSnapshot struct {
	ID          string
	Command     string
	Cwd         string
	Status      string // running | exited | killed
	ExitCode    int    // valid only when Status == "exited"
	JournalPath string // combined stdout+stderr log on disk
}

// tailChunk caps how many journal bytes one tail response carries; a client
// behind by more re-polls immediately (offset advances), so a fast-writing
// job streams in bounded slices.
const tailChunk = 512 * 1024

// tailBackfill is where a fresh tab (offset -1) starts in a large journal:
// the last window, not the whole gigabyte.
const tailBackfill = 256 * 1024

// handleJobTail is the terminal tab's poll: one request returns the job's
// current status AND the journal bytes since the client's offset, so a single
// cadence keeps both the output and the running/exited badge honest. Bytes
// ride base64 — a journal is raw process output, not guaranteed UTF-8 — and
// the client feeds them to its terminal renderer verbatim. offset=-1 means
// "from the tail": a huge log opens at its recent window instantly.
func (s *Server) handleJobTail(w http.ResponseWriter, r *http.Request) {
	id := r.PathValue("id")
	if !jobIDRe.MatchString(id) {
		http.Error(w, "bad job id", http.StatusBadRequest)
		return
	}
	job, err := s.FindJob(id)
	if err != nil {
		http.Error(w, err.Error(), http.StatusNotFound)
		return
	}

	offset, _ := strconv.ParseInt(r.URL.Query().Get("offset"), 10, 64)
	data, next, size := readJournal(job.JournalPath, offset)

	writeJSON(w, map[string]any{
		"id":        job.ID,
		"command":   job.Command,
		"cwd":       job.Cwd,
		"status":    job.Status,
		"exit_code": job.ExitCode,
		"data":      base64.StdEncoding.EncodeToString(data),
		"offset":    next,
		"size":      size,
	})
}

// readJournal returns up to tailChunk bytes of the journal from offset, the
// next offset, and the journal's current size. A negative offset starts at
// the recent window's edge; an offset past the end returns nothing.
func readJournal(path string, offset int64) (data []byte, next, size int64) {
	f, err := os.Open(path)
	if err != nil {
		return nil, max(offset, 0), 0
	}
	defer func() { _ = f.Close() }()
	st, err := f.Stat()
	if err != nil {
		return nil, max(offset, 0), 0
	}
	size = st.Size()
	if offset < 0 {
		offset = max(0, size-tailBackfill)
	}
	if offset >= size {
		return nil, offset, size
	}
	n := min(size-offset, tailChunk)
	buf := make([]byte, n)
	read, _ := f.ReadAt(buf, offset)
	return buf[:read], offset + int64(read), size
}

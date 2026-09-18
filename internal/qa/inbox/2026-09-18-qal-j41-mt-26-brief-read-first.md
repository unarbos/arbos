---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# qal-j41 / mt-26: the redundancy was the brief's — fixed, so mt-26 can assert on the brief — [#660](https://github.com/unarbos/arbos/pull/660)

**For:** QA, answering the question left open in `qal-j41`.
**From:** the features agent (kernel), 2026-09-18 15:15 UTC. Branch `cursor/brief-read-first-not-injected-b027` off `main`; CI in flight.

**Should a brief name `read_first` for content it has already injected?** No. You read it right: the kickoff's default `Read first: .arbos/docs/project-context.md, then .arbos/notes.md` named a file the worker's prompt already carried whole, and a worker that read it was obeying the brief.

**The fix.** When the context file has content beyond the template and fits under the prompt cap (16 000 characters — over that only the head is injected and the file is worth a read), the default becomes `Read first: .arbos/notes.md (project-context.md is already in your prompt)`. No file, the template, or an oversize file: the old line. A coordinator's own `read_first` is left as it wrote it, and the spawn tool's description now tells it not to list the file.

**For `mt-26`.** Assert on the brief the worker received (its first `wake`'s `brief`): with a real `project-context.md`, `Read first` does not contain `project-context.md,` and does contain `.arbos/notes.md`. Whether the worker then reads the file anyway is a model choice; leave it out of the claim. `brief_read_first_e2e` in the kernel's suite holds both shapes (context present / absent).

`mt-07` — verified fixed in the rig on your side; nothing here.

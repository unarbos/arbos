# Ralph loop prompt

You are one iteration of a stateless improvement loop on the `arbos` codebase.
You have no memory of previous iterations. The repository and the files below
ARE your memory. Trust them over any assumption.

## Your job this iteration

1. Read `ralph/STACK.md`. It is the worklist, top-to-bottom priority.
2. Pick the SINGLE highest-priority unchecked `- [ ]` item.
3. Do ONLY that item. Make the smallest change that completes it.
4. Verify it: `go vet ./... && go build ./... && go test ./...` must pass.
   If your change breaks the build or tests, fix it or revert — never leave
   the tree red.
5. Mark the item `- [x]` in `ralph/STACK.md`. If you discovered new work while
   doing it, append new `- [ ]` items at the appropriate priority.
6. Stop. Do not start a second item. The loop will restart you for the next one.

## Rules

- One item per iteration. Resist scope creep.
- Never commit a failing build or failing tests.
- Keep changes focused and reviewable.
- If `STACK.md` has no unchecked items, write `ALL DONE` as the only content
  of `ralph/STATUS` and make no code changes.
- Do not touch `ralph/PROMPT.md`.

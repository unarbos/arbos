+++
from = "agent:root"
kind = "brief"
wake = true
hops = 3
sent = "2026-09-17T09:42:45Z"
+++
Read first: .arbos/internal/sort_dataset.json
Task: Implement quicksort from scratch in Python and time it on the shared dataset.
Do: 
  1. Write .arbos/internal/sort_quicksort.py containing a function `sort(arr)` that implements quicksort yourself (no list.sort, no sorted(), no libraries).
  2. In the same file, under `if __name__ == "__main__":` load the 5000 numbers from .arbos/internal/sort_dataset.json, copy them, run sort() 5 times with time.perf_counter(), and print exactly two lines:
     ALGO quicksort
     BEST_MS <best of the 5 runs in milliseconds, 3 decimals>
  3. Assert your output equals sorted(data) so correctness is proven (using sorted() only for the assert is fine).
  4. Run it with `python3 .arbos/internal/sort_quicksort.py` and paste the real output.
  5. Raise recursion limit or use an iterative stack if needed — it must not crash.
Rules: No git branch or PR needed — this is a scratch benchmark, not a code fix. Do not touch the dataset file. Do not use sorted()/list.sort() inside your sort function.
Output: .arbos/internal/sort_quicksort.py
Report: The exact ALGO and BEST_MS lines from your real run, plus whether the correctness assert passed.

# Bounded Mac registry hydration

Run from a checkout with Rust dependencies cached, on macOS or within the
project's loopback-only Linux test namespace:

```sh
python3 .improve/perf/registry/check.py --out /tmp/arbos-registry-check --measure
```

The output directory must be fresh. No service, token store, Keychain or real
HTTP endpoint is used. Cargo builds locked/offline. macOS execution denies all
network; Linux requires an already isolated loopback-only namespace. Each arm
gets a new target directory, preventing cross-source compiled-test cache reuse.

The check extracts the selected production functions from the frozen upstream
base and current checkout. It uses actual project/chat/people DTOs, model mappers,
unread and local-edit merging, with scripted async JSON responses and small local
storage/event collaborators. It preserves typed results and request multisets for
success, bad/missing project details, missing members, people-list 404 fallback,
local edits/read marks and archived chats. Cancellation must drop all in-flight
requests and publish no partial registry. This checks the loading algorithm; it
is not a GUI, HTTP authentication or server integration test. The fixture's Rust
lockfile pins its own dependency closure; final application gates use the root
workspace lockfile.

Project metadata uses four ordered lanes, each fetching detail before members.
People hydration runs independently, so the per-refresh peak is five requests.
Unread and the project list initially run together. Project list and chat order,
member filtering, retry policy, cached fallbacks and the final publication remain
unchanged. Structured futures are dropped when their owning refresh is cancelled.
This bound applies to one refresh, not every HTTP request in the application.

`RESULTS.json` contains 21 randomized paired samples with a scripted 8 ms delay
per request, 12 projects and one people chat (27 requests total). Initial result:
330.809 → 90.993 ms median (3.64×), paired median saving 238.353 ms, 95% paired
bootstrap interval 230.700–241.418 ms, two-sided sign-test p=9.54e-7. Output
fingerprints match. Timer scheduling and generated fixtures affect these numbers;
no live-app latency reduction is asserted.

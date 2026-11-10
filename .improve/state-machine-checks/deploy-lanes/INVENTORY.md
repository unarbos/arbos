# AUX-H7 scoped state inventory

| Machine | Why/where | State storage | Risk and exit |
| --- | --- | --- | --- |
| Deployment request receipt | Hook atomic publish vs delayed worker completion; deploy-state.sh + worker handle_request and roll_image | Observed coherent snapshot; Acknowledged or Superseded decision under shared queue FD8 lock | High: old completion deleted new request. Formalized/tested at all three kind-file acknowledgement paths. Automatic publication freshness is a separately approved next delta. |
| Deployment effect admission | Independent relay, canary and Caddy processes; deploy-state.sh + worker + refresh-mac-download | Checking→Inspecting→Granted→Released or Deferred; actual owner is scoped kernel FD7, markers are independent observations | High: check/publication race admitted overlapping effects; timeout also deliberately allowed overlap. Formalized and pinned at actual effect boundaries, failure/cancellation and safe deadline. |

Manual image hold is already explicit and only an operator clears it; stale process-rules prose is corrected. systemd ActiveState is already explicit. Per-target remote progress, pin, refusal and quota observations are independent facts, not illegal combinations in one controller. Dry tick was a filesystem ownership violation and is fixed at scratch establishment, rather than inventing another phase enum.

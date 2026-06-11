import { argsPreview } from "@/lib/format";
import type { PendingApproval } from "@/lib/transcript";

/** The pending tool approval: the call, the reason, and Run / Skip buttons. */
export function ApprovalCard({
  approval,
  onAnswer,
}: {
  approval: PendingApproval;
  onAnswer: (approved: boolean) => void;
}) {
  return (
    <div className="rounded-[10px] border border-line bg-card px-3 py-2.5">
      <div className="break-words font-mono text-[11.5px]">
        <span className="text-bright">{approval.call.Name}</span>{" "}
        <span className="text-muted">{argsPreview(approval.call, 200)}</span>
      </div>
      {approval.reason && (
        <div className="mt-0.5 text-[12px] text-muted">{approval.reason}</div>
      )}
      <div className="mt-2 flex items-center gap-2">
        <button
          type="button"
          onClick={() => onAnswer(true)}
          className="cursor-pointer rounded-md bg-btn px-3 py-0.5 text-[12px] font-medium text-canvas transition-opacity hover:opacity-90"
        >
          Run
        </button>
        <button
          type="button"
          onClick={() => onAnswer(false)}
          className="cursor-pointer rounded-md border border-line px-3 py-0.5 text-[12px] text-muted transition-colors hover:text-text"
        >
          Skip
        </button>
        <span className="ml-1 text-[11px] text-faint select-none">y / n</span>
      </div>
    </div>
  );
}

import { ArrowUp, Pencil, Trash2 } from "lucide-react";

import { PartImages } from "./PartImages";
import type { ContentBlock } from "@/lib/types";

export interface QueuedMessage {
  qid: number;
  text: string;
  parts: ContentBlock[];
}

/**
 * Messages waiting their turn, Cursor-style: quiet cards under the transcript
 * with hover affordances — edit (back into the composer), push (send now,
 * steering the in-flight turn), delete.
 */
export function QueuedMessages({
  queue,
  onPush,
  onEdit,
  onDelete,
}: {
  queue: QueuedMessage[];
  onPush: (qid: number) => void;
  onEdit: (qid: number) => void;
  onDelete: (qid: number) => void;
}) {
  return (
    <div className="space-y-1">
      {queue.map((m) => (
        <div
          key={m.qid}
          className="group flex items-center gap-2 rounded-md border border-line/60 bg-card/60 px-3 py-1.5"
        >
          <span className="flex min-w-0 flex-1 items-center gap-1.5">
            <PartImages
              parts={m.parts}
              className="size-7 shrink-0 rounded-sm border border-line/60 object-cover"
            />
            {m.text && <span className="min-w-0 truncate text-muted">{m.text}</span>}
          </span>
          <span className="flex shrink-0 items-center gap-1 opacity-0 transition-opacity group-hover:opacity-100">
            <button
              type="button"
              onClick={() => onEdit(m.qid)}
              title="Edit"
              className="flex size-5 cursor-pointer items-center justify-center rounded text-faint transition-colors hover:bg-hover hover:text-text"
            >
              <Pencil size={11} />
            </button>
            <button
              type="button"
              onClick={() => onPush(m.qid)}
              title="Send now"
              className="flex size-5 cursor-pointer items-center justify-center rounded text-faint transition-colors hover:bg-hover hover:text-text"
            >
              <ArrowUp size={12} />
            </button>
            <button
              type="button"
              onClick={() => onDelete(m.qid)}
              title="Delete"
              className="flex size-5 cursor-pointer items-center justify-center rounded text-faint transition-colors hover:bg-hover hover:text-red"
            >
              <Trash2 size={11} />
            </button>
          </span>
        </div>
      ))}
    </div>
  );
}

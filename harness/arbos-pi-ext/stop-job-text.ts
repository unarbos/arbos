/** What `Agent::hook_stop_job` answers with, as the fields this renderer must not lose. */
export type StopJobResult = {
  stopped?: {
    job_id?: string;
    title?: string;
    state?: string;
    processes_checked?: boolean;
    processes_killed?: number;
    processes_still_alive?: number[];
  }[];
  unnamed_processes_checked?: boolean;
  unnamed_processes_killed?: number;
  unnamed_processes_still_alive?: number[];
  unconfirmed_because?: string;
  sessions_not_read?: string[];
  note?: string;
};

/**
 * Whether a sweep read the kernel, and what it found — rendered only from the fields that are
 * actually there.
 *
 * An absent field means agentd declined to claim anything, and it has to stay absent here. A
 * missing `processes_checked` rendered as "checked no" would invent a failure, and rendered as
 * "checked yes" would be bug AE's sentence with the numbers filled in. Mirrors
 * `Sweep::checked`'s own convention: no field rather than a convenient one.
 */
function sweepClause(checked?: boolean, killed?: number, alive?: number[]): string {
  const bits: string[] = [];
  if (typeof checked === "boolean") bits.push(`checked ${checked ? "yes" : "no"}`);
  if (typeof killed === "number") bits.push(`killed ${killed}`);
  if (Array.isArray(alive)) bits.push(`still alive ${alive.length ? alive.join(", ") : "none"}`);
  return bits.join(", ");
}

/**
 * The text a `stop_job` answer reaches the chat as.
 *
 * **This is the only copy of that evidence anyone sees, which is why it is a named function with
 * a test of its own.** #2867 landed on the condition that an unconfirmed sweep's cause and its
 * skipped session ids reach an observer, and it did not: this renderer was `${states}. ${note}`,
 * so `unconfirmed_because`, `sessions_not_read`, `processes_checked` and the `unnamed_*` fields
 * were dropped here, before the chat row, the offloaded tool-output blob, the relay audit trail
 * or `.arbos/audit.jsonl` could carry any of them. agentd's struct was right the whole time and
 * a struct-level test said so, which is exactly how the gap shipped.
 *
 * So the fields go into the text the model reads. The `note` still leads, because it is the
 * sentence agentd wrote for the model to act on; the evidence follows it, for the model to
 * repeat and for a canary to match on.
 */
export function stopJobText(result: StopJobResult | undefined): string {
  const stopped = result?.stopped ?? [];
  const states = stopped
    .map((j) => {
      const sweep = sweepClause(j.processes_checked, j.processes_killed, j.processes_still_alive);
      return `${j.title ?? j.job_id} — state ${j.state ?? "unknown"}${sweep ? ` (processes ${sweep})` : ""}`;
    })
    .join("; ") || "nothing";
  const parts = [`${states}.`, result?.note ?? ""];
  if (result?.unconfirmed_because) {
    const sessions = result.sessions_not_read ?? [];
    const named = sessions.length ? `; sessions not read: ${sessions.join(", ")}` : "";
    parts.push(`Unconfirmed because: ${result.unconfirmed_because}${named}.`);
  }
  const unnamed = sweepClause(
    result?.unnamed_processes_checked,
    result?.unnamed_processes_killed,
    result?.unnamed_processes_still_alive,
  );
  if (unnamed) parts.push(`Work belonging to no job row: ${unnamed}.`);
  return parts.map((p) => p.trim()).filter(Boolean).join(" ");
}

import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { decodeReply, type Reply, type Request } from "./protocol.ts";

type Pending = {
  id: number;
  op: Request["op"];
  resolve: (reply: Reply) => void;
  reject: (error: Error) => void;
  timer: NodeJS.Timeout;
};

/** Sequential JSONL adapter. A failed transport is never reused as a passing oracle. */
export class Bridge {
  private readonly child: ChildProcessWithoutNullStreams;
  private readonly exited: Promise<void>;
  private readonly timeoutMs: number;
  private readonly maxLineBytes: number;
  private tail: Promise<unknown> = Promise.resolve();
  private pending: Pending | undefined;
  private buffer = "";
  private nextId = 0;
  private failed: Error | undefined;
  private closing = false;

  constructor(command: string, args: readonly string[] = [], options: { timeoutMs?: number; maxLineBytes?: number } = {}) {
    this.timeoutMs = options.timeoutMs ?? 5000;
    this.maxLineBytes = options.maxLineBytes ?? 1024 * 1024;
    this.child = spawn(command, [...args], { stdio: "pipe", shell: false });
    this.exited = new Promise((resolve) => {
      this.child.once("close", () => {
        if (!this.closing) this.fail("Native bridge exited unexpectedly");
        resolve();
      });
      this.child.once("error", () => {
        this.fail("Native bridge failed to start");
        resolve();
      });
    });
    this.child.stdin.on("error", () => this.fail("Native bridge input failed"));
    this.child.stdout.setEncoding("utf8");
    this.child.stdout.on("data", (chunk: string) => this.consume(chunk));
    // Drain diagnostics without exposing input/parser content or accumulating unbounded logs.
    this.child.stderr.on("data", () => {});
  }

  request(request: Request): Promise<Reply> {
    const result = this.tail.then(() => this.send(request));
    this.tail = result.catch(() => undefined);
    return result;
  }

  private send(request: Request): Promise<Reply> {
    if (this.failed) return Promise.reject(this.failed);
    if (this.closing) return Promise.reject(new Error("Native bridge is closed"));
    const id = this.nextId++;
    const line = JSON.stringify({ id, ...request });
    if (Buffer.byteLength(line) > 65536) return Promise.reject(new Error("Native bridge request exceeds limit"));
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => this.fail("Native bridge response deadline exceeded"), this.timeoutMs);
      this.pending = { id, op: request.op, resolve, reject, timer };
      this.child.stdin.write(line + "\n");
    });
  }

  private consume(chunk: string): void {
    if (this.failed || this.closing) return;
    this.buffer += chunk;
    if (Buffer.byteLength(this.buffer) > this.maxLineBytes) {
      this.fail("Native bridge output exceeds limit");
      return;
    }
    for (;;) {
      const newline = this.buffer.indexOf("\n");
      if (newline < 0) return;
      const line = this.buffer.slice(0, newline);
      this.buffer = this.buffer.slice(newline + 1);
      const pending = this.pending;
      if (!pending) {
        this.fail("Native bridge sent an unsolicited response");
        return;
      }
      try {
        const reply = decodeReply(pending.op, JSON.parse(line));
        if (reply.id !== pending.id) throw new Error("Mismatched response id");
        clearTimeout(pending.timer);
        this.pending = undefined;
        pending.resolve(reply);
      } catch {
        this.fail("Native bridge returned an invalid response");
        return;
      }
    }
  }

  private fail(message: string): void {
    this.failed ??= new Error(message);
    if (this.pending) {
      clearTimeout(this.pending.timer);
      this.pending.reject(this.failed);
      this.pending = undefined;
    }
    this.child.kill();
  }

  async close(): Promise<void> {
    this.closing = true;
    if (this.pending) {
      clearTimeout(this.pending.timer);
      this.pending.reject(new Error("Native bridge closed with a pending request"));
      this.pending = undefined;
    }
    this.child.stdin.end();
    const timer = setTimeout(() => this.child.kill("SIGKILL"), 1000);
    try {
      await this.exited;
    } finally {
      clearTimeout(timer);
    }
  }
}

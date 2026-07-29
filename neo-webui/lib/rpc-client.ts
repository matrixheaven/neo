import { EventEmitter } from "events";
import { spawn, ChildProcessWithoutNullStreams } from "child_process";

// ── RPC wire types ──────────────────────────────────────────────────────────

interface RpcMessage {
  type: "request" | "response" | "notification";
}

interface RpcResponse extends RpcMessage {
  type: "response";
  id: string;
  result?: unknown;
  error?: { code: string; message: string; data?: unknown };
}

interface RpcNotification extends RpcMessage {
  type: "notification";
  method: string;
  params: unknown;
}

interface RpcRequest {
  type: "request";
  id: string;
  method: string;
  params: unknown;
}

// ── Pending call ───────────────────────────────────────────────────────────

interface PendingCall {
  resolve: (value: unknown) => void;
  reject: (reason: Error) => void;
}

// ── NeoRpcProcess ──────────────────────────────────────────────────────────

export class NeoRpcProcess extends EventEmitter {
  private process: ChildProcessWithoutNullStreams;
  private pending: Map<string, PendingCall> = new Map();
  private nextId = 0;
  private buffer = "";
  private destroyed = false;

  constructor(neoBinary: string = "neo", workspaceDir?: string) {
    super();

    const args = ["rpc"];
    const options: import("child_process").SpawnOptions = {
      stdio: ["pipe", "pipe", "pipe"],
    };
    if (workspaceDir) {
      options.cwd = workspaceDir;
    }

    this.process = spawn(neoBinary, args, options);

    this.process.stdout.on("data", (chunk: Buffer) => {
      this.handleStdout(chunk.toString("utf-8"));
    });

    this.process.stderr.on("data", (chunk: Buffer) => {
      this.emit("stderr", chunk.toString("utf-8"));
    });

    this.process.on("close", (code: number | null) => {
      this.destroyed = true;
      this.emit("close", code);
      // Reject all pending calls
      for (const [id, call] of this.pending) {
        call.reject(new Error(`neo rpc exited (code ${code}) before call ${id}`));
      }
      this.pending.clear();
    });

    this.process.on("error", (err: Error) => {
      this.destroyed = true;
      this.emit("error", err);
    });
  }

  // ── public API ─────────────────────────────────────────────────────────

  call(method: string, params?: unknown): Promise<unknown> {
    if (this.destroyed) {
      return Promise.reject(new Error("NeoRpcProcess is destroyed"));
    }

    const id = String(++this.nextId);
    const request: RpcRequest = {
      type: "request",
      id,
      method,
      params: params ?? {},
    };

    return new Promise<unknown>((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      const line = JSON.stringify(request) + "\n";
      this.process.stdin.write(line, (err) => {
        if (err) {
          this.pending.delete(id);
          reject(err);
        }
      });
    });
  }

  destroy(): void {
    if (this.destroyed) return;
    this.destroyed = true;
    this.process.kill();
  }

  // ── internal ───────────────────────────────────────────────────────────

  private handleStdout(data: string): void {
    this.buffer += data;
    const lines = this.buffer.split("\n");
    // Keep the last (potentially incomplete) line in the buffer
    this.buffer = lines.pop() ?? "";

    for (const line of lines) {
      const trimmed = line.trim();
      if (!trimmed) continue;

      let message: RpcMessage;
      try {
        message = JSON.parse(trimmed);
      } catch {
        // Malformed JSON — ignore as the Rust side handles parse errors
        continue;
      }

      if (message.type === "response") {
        const response = message as RpcResponse;
        const pending = this.pending.get(response.id);
        if (!pending) continue;
        this.pending.delete(response.id);

        if (response.error) {
          pending.reject(
            new Error(
              `RPC error ${response.error.code}: ${response.error.message}`,
            ),
          );
        } else {
          pending.resolve(response.result);
        }
      } else if (message.type === "notification") {
        const notification = message as RpcNotification;
        this.emit("notification", notification.method, notification.params);
      }
    }
  }
}

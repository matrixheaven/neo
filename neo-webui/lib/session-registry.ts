import { NeoRpcProcess } from "./rpc-client";

// ── SessionEntry ──────────────────────────────────────────────────────────

export interface SessionEntry {
  process: NeoRpcProcess;
  createdAt: number;
  lastActivityAt: number;
}

// ── SessionRegistry ───────────────────────────────────────────────────────

const IDLE_TIMEOUT_MS = 30 * 60 * 1000; // 30 minutes

export class SessionRegistry {
  private sessions: Map<string, SessionEntry> = new Map();
  private startLocks: Map<string, Promise<SessionEntry>> = new Map();
  private cleanupTimer: ReturnType<typeof setInterval> | null = null;

  constructor() {
    this.cleanupTimer = setInterval(() => {
      this.cleanupIdle();
    }, 60_000);
  }

  // ── ensure ────────────────────────────────────────────────────────────

  async ensure(sessionId: string): Promise<SessionEntry> {
    // Fast path: already running
    const existing = this.sessions.get(sessionId);
    if (existing) {
      existing.lastActivityAt = Date.now();
      return existing;
    }

    // Check if a start is already in flight for this sessionId
    const inFlight = this.startLocks.get(sessionId);
    if (inFlight) {
      const entry = await inFlight;
      entry.lastActivityAt = Date.now();
      return entry;
    }

    // Start a new process under the start-lock
    const startPromise = this.startProcess(sessionId);
    this.startLocks.set(sessionId, startPromise);

    try {
      const entry = await startPromise;
      return entry;
    } finally {
      this.startLocks.delete(sessionId);
    }
  }

  // ── internal ───────────────────────────────────────────────────────────

  private async startProcess(sessionId: string): Promise<SessionEntry> {
    const process = new NeoRpcProcess();
    const now = Date.now();
    const entry: SessionEntry = {
      process,
      createdAt: now,
      lastActivityAt: now,
    };

    process.on("close", () => {
      this.sessions.delete(sessionId);
    });

    this.sessions.set(sessionId, entry);
    return entry;
  }

  private cleanupIdle(): void {
    const now = Date.now();
    for (const [id, entry] of this.sessions) {
      if (now - entry.lastActivityAt >= IDLE_TIMEOUT_MS) {
        entry.process.destroy();
        this.sessions.delete(id);
      }
    }
  }

  destroy(): void {
    if (this.cleanupTimer) {
      clearInterval(this.cleanupTimer);
      this.cleanupTimer = null;
    }
    for (const entry of this.sessions.values()) {
      entry.process.destroy();
    }
    this.sessions.clear();
    this.startLocks.clear();
  }
}

// ── singleton ─────────────────────────────────────────────────────────────

export const sessionRegistry = new SessionRegistry();

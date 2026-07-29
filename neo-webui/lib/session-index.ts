import * as fs from 'fs';
import * as path from 'path';
import * as os from 'os';

// ── Wire path decoding ────────────────────────────────────────────────────

interface NativePathWire {
  kind: 'windows' | 'unix';
  units?: string;  // base64 of UTF-16LE bytes
  bytes?: string;  // base64 of raw bytes
}

function decodeWorkdir(raw: unknown): string | null {
  if (typeof raw === 'string') return raw;
  if (raw === null || typeof raw !== 'object') return null;
  const wire = raw as NativePathWire;
  try {
    if (wire.kind === 'windows' && typeof wire.units === 'string') {
      const buf = Buffer.from(wire.units, 'base64');
      const codeUnits: number[] = [];
      for (let i = 0; i + 1 < buf.length; i += 2) {
        codeUnits.push(buf.readUInt16LE(i));
      }
      return String.fromCharCode(...codeUnits);
    }
    if (wire.kind === 'unix' && typeof wire.bytes === 'string') {
      return Buffer.from(wire.bytes, 'base64').toString('utf-8');
    }
  } catch { /* ignore malformed entries */ }
  return null;
}

function decodeSessionDir(raw: unknown): string | null {
  return decodeWorkdir(raw);
}

// ── Types ──────────────────────────────────────────────────────────────────

export interface SessionSummary {
  id: string;
  session_dir: string;
  workdir: string;
}

export interface SessionGroup {
  workdir: string;
  sessions: SessionSummary[];
}

// ── Helpers ────────────────────────────────────────────────────────────────

function neoHome(): string {
  return process.env.NEO_HOME ?? path.join(os.homedir(), '.neo');
}

// ── List all sessions ──────────────────────────────────────────────────────

export function listAllSessions(): SessionGroup[] {
  const indexPath = path.join(neoHome(), 'session_index.jsonl');
  if (!fs.existsSync(indexPath)) return [];
  const raw = fs.readFileSync(indexPath, 'utf-8').trim();
  if (!raw) return [];

  const summaries: SessionSummary[] = [];
  for (const line of raw.split('\n')) {
    try {
      const obj = JSON.parse(line);
      if (typeof obj.session_id !== 'string') continue;
      const workdir = decodeWorkdir(obj.workdir);
      if (!workdir) continue;
      const sessionDir = decodeSessionDir(obj.session_dir) ?? '';
      summaries.push({ id: obj.session_id, session_dir: sessionDir, workdir });
    } catch { /* skip malformed lines */ }
  }

  const groups = new Map<string, SessionSummary[]>();
  for (const s of summaries) {
    const existing = groups.get(s.workdir) ?? [];
    existing.push(s);
    groups.set(s.workdir, existing);
  }
  return [...groups.entries()].map(([workdir, sessions]) => ({ workdir, sessions }));
}

// ── Find single session workdir ─────────────────────────────────────────────

export function findSessionWorkdir(sessionId: string): string | null {
  const indexPath = path.join(neoHome(), 'session_index.jsonl');
  if (!fs.existsSync(indexPath)) return null;
  const raw = fs.readFileSync(indexPath, 'utf-8').trim();
  if (!raw) return null;
  for (const line of raw.split('\n')) {
    try {
      const obj = JSON.parse(line);
      if (obj.session_id === sessionId) {
        return decodeWorkdir(obj.workdir);
      }
    } catch { /* skip malformed lines */ }
  }
  return null;
}

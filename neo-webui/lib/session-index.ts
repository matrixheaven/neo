import * as fs from 'fs';
import * as path from 'path';
import * as os from 'os';

interface IndexEntry {
  session_id: string;
  session_dir: string;
  workdir: string;
}

export interface SessionGroup {
  workdir: string;
  sessions: IndexEntry[];
}

function neoHome(): string {
  return process.env.NEO_HOME ?? path.join(os.homedir(), '.neo');
}

export function listAllSessions(): SessionGroup[] {
  const indexPath = path.join(neoHome(), 'session_index.jsonl');
  if (!fs.existsSync(indexPath)) return [];
  const raw = fs.readFileSync(indexPath, 'utf-8').trim();
  if (!raw) return [];
  const entries: IndexEntry[] = raw.split('\n').map(line => JSON.parse(line));
  const groups = new Map<string, IndexEntry[]>();
  for (const entry of entries) {
    const existing = groups.get(entry.workdir) ?? [];
    existing.push(entry);
    groups.set(entry.workdir, existing);
  }
  return [...groups.entries()].map(([workdir, sessions]) => ({ workdir, sessions }));
}

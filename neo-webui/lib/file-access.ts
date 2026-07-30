import * as fs from 'fs';
import * as path from 'path';

const DEFAULT_WORKSPACE_ROOT = process.cwd();

export function resolveFile(subpath: string, root?: string): { valid: boolean; fullPath?: string; error?: string } {
  const workspaceRoot = root || DEFAULT_WORKSPACE_ROOT;
  const normalized = path.normalize(subpath).replace(/^[/\\]+/, '');
  const fullPath = path.resolve(workspaceRoot, normalized);

  // Security: must be within workspace
  if (!fullPath.startsWith(workspaceRoot)) {
    return { valid: false, error: 'Path escapes workspace' };
  }

  return { valid: true, fullPath };
}

export function readTextFile(fullPath: string, requestSubpath?: string): { content?: string; error?: string } {
  try {
    const stat = fs.statSync(fullPath);
    if (stat.isDirectory()) {
      // List directory
      const prefix = requestSubpath ? requestSubpath.replace(/\\/g, '/') + '/' : '';
      const entries = fs.readdirSync(fullPath).map(name => {
        const p = path.join(fullPath, name);
        const s = fs.statSync(p);
        return { name, path: prefix + name, isDirectory: s.isDirectory(), size: s.size };
      });
      return { content: JSON.stringify(entries) };
    }
    if (stat.size > 1024 * 1024) { // 1MB limit
      return { error: 'File too large (>1MB)' };
    }
    const content = fs.readFileSync(fullPath, 'utf-8');
    return { content };
  } catch (e: any) {
    return { error: e.message ?? 'File not found' };
  }
}

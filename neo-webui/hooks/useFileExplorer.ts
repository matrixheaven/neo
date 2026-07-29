'use client';
import { useState, useCallback } from 'react';

export interface FileNode {
  name: string;
  isDirectory: boolean;
  size: number;
  path: string;
  children?: FileNode[];
}

export function useFileExplorer(workspace: string | null) {
  const [files, setFiles] = useState<FileNode[]>([]);
  const [expandedDirs, setExpandedDirs] = useState<Set<string>>(new Set());
  const [loading, setLoading] = useState(false);

  const loadDirectory = useCallback(async (dirPath: string = '') => {
    if (dirPath == null) return;
    if (!workspace) return;
    setLoading(true);
    try {
      const base = dirPath ? `/api/files/${dirPath}` : '/api/files';
      const url = `${base}?root=${encodeURIComponent(workspace)}`;
      const res = await fetch(url);
      const data = await res.json();
      if (data.content) {
        const raw: FileNode[] = JSON.parse(data.content);
        // Deduplicate by path
        const seen = new Map<string, FileNode>();
        for (const n of raw) seen.set(n.path, n);
        setFiles([...seen.values()]);
      }
    } catch (e) {
      console.error('Failed to load directory:', e);
    } finally {
      setLoading(false);
    }
  }, [workspace]);

  const toggleDir = useCallback((path: string) => {
    setExpandedDirs(prev => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  }, []);

  return { files, expandedDirs, loading, loadDirectory, toggleDir };
}

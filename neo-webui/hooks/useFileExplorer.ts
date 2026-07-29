'use client';
import { useState, useCallback } from 'react';

export interface FileNode {
  name: string;
  isDirectory: boolean;
  size: number;
  path: string;
  children?: FileNode[];
}

export function useFileExplorer() {
  const [files, setFiles] = useState<FileNode[]>([]);
  const [expandedDirs, setExpandedDirs] = useState<Set<string>>(new Set());
  const [loading, setLoading] = useState(false);

  const loadDirectory = useCallback(async (dirPath: string = '') => {
    setLoading(true);
    try {
      const res = await fetch(`/api/files/${dirPath}`);
      const data = await res.json();
      if (data.content) {
        const nodes: FileNode[] = JSON.parse(data.content);
        setFiles(nodes);
      }
    } catch (e) {
      console.error('Failed to load directory:', e);
    } finally {
      setLoading(false);
    }
  }, []);

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

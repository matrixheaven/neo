'use client';
import React, { useEffect } from 'react';
import { useFileExplorer, type FileNode } from '@/hooks/useFileExplorer';

interface FileExplorerProps {
  onOpenFile: (path: string) => void;
}

export function FileExplorer({ onOpenFile }: FileExplorerProps) {
  const { files, expandedDirs, loading, loadDirectory, toggleDir } = useFileExplorer();

  useEffect(() => {
    loadDirectory('');
  }, [loadDirectory]);

  const renderNode = (node: FileNode, depth: number = 0) => {
    const isExpanded = expandedDirs.has(node.path);

    return (
      <div key={node.path}>
        <div
          onClick={() => {
            if (node.isDirectory) {
              toggleDir(node.path);
              loadDirectory(node.path);
            } else {
              onOpenFile(node.path);
            }
          }}
          style={{
            padding: `2px ${8 + depth * 16}px 2px ${8 + depth * 16}px`,
            cursor: 'pointer',
            fontSize: 'var(--font-size-sm)',
            color: 'var(--color-text-secondary)',
            display: 'flex',
            alignItems: 'center',
            gap: '4px',
            userSelect: 'none',
          }}
        >
          <span>{node.isDirectory ? (isExpanded ? '📂' : '📁') : '📄'}</span>
          <span style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
            {node.name}
          </span>
        </div>
        {node.isDirectory && isExpanded && node.children?.map(child => renderNode(child, depth + 1))}
      </div>
    );
  };

  return (
    <div style={{
      width: 220,
      height: '100%',
      background: 'var(--color-bg-secondary)',
      borderRight: '1px solid var(--color-border)',
      overflowY: 'auto',
      padding: 'var(--space-sm) 0',
    }}>
      <div style={{
        padding: 'var(--space-sm) var(--space-md)',
        fontSize: 'var(--font-size-xs, 11px)',
        color: 'var(--color-text-tertiary)',
        textTransform: 'uppercase',
        letterSpacing: '0.5px',
        fontWeight: 600,
      }}>
        Files
      </div>
      {loading && <div style={{ padding: 'var(--space-md)', color: 'var(--color-text-tertiary)', fontSize: 'var(--font-size-sm)' }}>Loading...</div>}
      {files.map(node => renderNode(node))}
    </div>
  );
}

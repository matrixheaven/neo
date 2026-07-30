'use client';
import React, { useState } from 'react';
import type { SessionState } from '@/lib/types';

interface SessionNodeProps {
  session: SessionState;
  isActive: boolean;
  onClick: () => void;
  onRename: (name: string) => void;
  onExport: (format: 'html' | 'json') => void;
}

export function SessionNode({ session, isActive, onClick, onRename, onExport }: SessionNodeProps) {
  const [showMenu, setShowMenu] = useState(false);
  const [editing, setEditing] = useState(false);
  const [name, setName] = useState(session.name || session.id);

  const handleContextMenu = (e: React.MouseEvent) => {
    e.preventDefault();
    setShowMenu(true);
  };

  return (
    <div
      onClick={onClick}
      onContextMenu={handleContextMenu}
      style={{
        padding: 'var(--space-sm) var(--space-md)',
        cursor: 'pointer',
        fontSize: 'var(--font-size-sm)',
        color: isActive ? 'var(--color-text-primary)' : 'var(--color-text-secondary)',
        background: isActive ? 'var(--color-accent-subtle)' : 'transparent',
        borderRadius: 'var(--radius-sm)',
        margin: '2px var(--space-sm)',
        display: 'flex',
        alignItems: 'center',
        gap: 'var(--space-sm)',
        position: 'relative',
      }}
      title={session.id}
    >
      <span style={{ opacity: 0.5 }}>💬</span>
      {editing ? (
        <input
          value={name}
          onChange={e => setName(e.target.value)}
          onBlur={() => { setEditing(false); onRename(name); }}
          onKeyDown={e => { if (e.key === 'Enter') { setEditing(false); onRename(name); } }}
          autoFocus
          style={{
            flex: 1,
            background: 'var(--color-bg-primary)',
            border: '1px solid var(--color-border)',
            borderRadius: 'var(--radius-sm)',
            padding: '2px var(--space-sm)',
            color: 'var(--color-text-primary)',
            fontSize: 'var(--font-size-sm)',
          }}
        />
      ) : (
        <span style={{ flex: 1, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
          {session.name || session.id.slice(0, 12)}
        </span>
      )}

      {showMenu && (
        <>
          <div
            style={{ position: 'fixed', inset: 0, zIndex: 99 }}
            onClick={() => setShowMenu(false)}
          />
          <div style={{
            position: 'absolute',
            top: '100%',
            right: 0,
            zIndex: 100,
            background: 'var(--color-bg-primary)',
            border: '1px solid var(--color-border)',
            borderRadius: 'var(--radius-md)',
            boxShadow: 'var(--shadow-md)',
            minWidth: 140,
            overflow: 'hidden',
          }}>
            <MenuItem onClick={() => { setEditing(true); setShowMenu(false); }}>Rename</MenuItem>
            <MenuItem onClick={() => { onExport('html'); setShowMenu(false); }}>Export HTML</MenuItem>
            <MenuItem onClick={() => { onExport('json'); setShowMenu(false); }}>Export JSON</MenuItem>
          </div>
        </>
      )}
    </div>
  );
}

function MenuItem({ onClick, children }: { onClick: () => void; children: React.ReactNode }) {
  return (
    <div
      onClick={onClick}
      style={{
        padding: 'var(--space-sm) var(--space-md)',
        cursor: 'pointer',
        fontSize: 'var(--font-size-sm)',
      }}
    >
      {children}
    </div>
  );
}

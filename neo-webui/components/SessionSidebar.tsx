'use client';
import React, { useEffect, useState } from 'react';
import { SessionNode } from './SessionNode';
import type { SessionState } from '@/lib/types';

interface SessionSidebarProps {
  activeSessionId: string | null;
  onSelectSession: (id: string) => void;
  onToggleFileExplorer: () => void;
  showFileExplorer: boolean;
}

export function SessionSidebar({ activeSessionId, onSelectSession, onToggleFileExplorer, showFileExplorer }: SessionSidebarProps) {
  const [groups, setGroups] = useState<Array<{ workdir: string; sessions: SessionState[] }>>([]);
  const [search, setSearch] = useState('');

  useEffect(() => {
    fetch('/api/sessions')
      .then(r => r.json())
      .then(data => setGroups(data.groups || []))
      .catch(console.error);
  }, []);

  const filtered = groups.map(g => ({
    ...g,
    sessions: g.sessions.filter(s => {
      const term = search.toLowerCase();
      return (s.name || s.id).toLowerCase().includes(term);
    }),
  })).filter(g => g.sessions.length > 0);

  const handleRename = async (sessionId: string, name: string) => {
    await fetch(`/api/sessions/${sessionId}`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ name }),
    });
    // Refresh
    setGroups(prev => prev.map(g => ({
      ...g,
      sessions: g.sessions.map(s => s.id === sessionId ? { ...s, name } : s),
    })));
  };

  const handleExport = async (sessionId: string, format: 'html' | 'json') => {
    const res = await fetch(`/api/sessions/${sessionId}/export?format=${format}`);
    if (format === 'json') {
      const blob = await res.blob();
      const url = URL.createObjectURL(blob);
      const a = document.createElement('a');
      a.href = url;
      a.download = `${sessionId}.json`;
      a.click();
    } else {
      const text = await res.text();
      const blob = new Blob([text], { type: 'text/html' });
      const url = URL.createObjectURL(blob);
      const a = document.createElement('a');
      a.href = url;
      a.download = `${sessionId}.html`;
      a.click();
    }
  };

  return (
    <div style={{
      width: 'var(--sidebar-width)',
      height: '100%',
      display: 'flex',
      flexDirection: 'column',
      background: 'var(--color-bg-secondary)',
      borderRight: '1px solid var(--color-border)',
    }}>
      <div style={{
        padding: 'var(--space-md)',
        borderBottom: '1px solid var(--color-border)',
        display: 'flex',
        gap: 'var(--space-sm)',
      }}>
        <input
          type="text"
          placeholder="Search sessions..."
          value={search}
          onChange={e => setSearch(e.target.value)}
          style={{
            flex: 1,
            background: 'var(--color-bg-primary)',
            border: '1px solid var(--color-border)',
            borderRadius: 'var(--radius-sm)',
            padding: 'var(--space-sm) var(--space-md)',
            color: 'var(--color-text-primary)',
            fontSize: 'var(--font-size-sm)',
            outline: 'none',
          }}
        />
        <button
          onClick={onToggleFileExplorer}
          style={{
            background: showFileExplorer ? 'var(--color-accent)' : 'var(--color-bg-tertiary)',
            border: '1px solid var(--color-border)',
            borderRadius: 'var(--radius-sm)',
            padding: 'var(--space-sm) var(--space-md)',
            color: showFileExplorer ? 'white' : 'var(--color-text-secondary)',
            cursor: 'pointer',
            fontSize: 'var(--font-size-sm)',
          }}
        >
          📁
        </button>
      </div>

      <div style={{ flex: 1, overflowY: 'auto', padding: 'var(--space-sm) 0' }}>
        {filtered.map(group => (
          <div key={group.workdir}>
            <div style={{
              padding: 'var(--space-sm) var(--space-md)',
              fontSize: 'var(--font-size-xs, 11px)',
              color: 'var(--color-text-tertiary)',
              textTransform: 'uppercase',
              letterSpacing: '0.5px',
              fontWeight: 600,
            }}>
              {group.workdir.split(/[/\\]/).pop() || group.workdir}
            </div>
            {group.sessions.map(session => (
              <SessionNode
                key={session.id}
                session={session}
                isActive={session.id === activeSessionId}
                onClick={() => onSelectSession(session.id)}
                onRename={(name) => handleRename(session.id, name)}
                onExport={(format) => handleExport(session.id, format)}
              />
            ))}
          </div>
        ))}
      </div>
    </div>
  );
}

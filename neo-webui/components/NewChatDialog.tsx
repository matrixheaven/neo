'use client';
import React, { useState, useCallback } from 'react';
import { useRouter } from 'next/navigation';

interface NewChatDialogProps {
  onClose: () => void;
}

export function NewChatDialog({ onClose }: NewChatDialogProps) {
  const router = useRouter();
  const [workdir, setWorkdir] = useState('');
  const [isCreating, setIsCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const handleCreate = useCallback(async () => {
    const trimmed = workdir.trim();
    if (!trimmed) {
      setError('Please enter a working directory');
      return;
    }
    setIsCreating(true);
    setError(null);
    try {
      const res = await fetch('/api/sessions', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ workdir: trimmed }),
      });
      if (!res.ok) {
        const data = await res.json();
        setError(data.error || 'Failed to create session');
        return;
      }
      const data = await res.json();
      router.push(`/session/${data.session_id}`);
    } catch (e: any) {
      setError(e.message || 'Failed to create session');
    } finally {
      setIsCreating(false);
    }
  }, [workdir, router]);

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Enter') handleCreate();
    if (e.key === 'Escape') onClose();
  };

  return (
    <div
      style={{
        position: 'fixed',
        inset: 0,
        zIndex: 200,
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
      }}
    >
      <div
        onClick={onClose}
        style={{ position: 'absolute', inset: 0, background: 'rgba(0,0,0,0.5)' }}
      />
      <div
        style={{
          position: 'relative',
          width: 480,
          background: 'var(--color-bg-primary)',
          borderRadius: 'var(--radius-lg)',
          boxShadow: 'var(--shadow-lg)',
          display: 'flex',
          flexDirection: 'column',
        }}
      >
        <div
          style={{
            padding: 'var(--space-md) var(--space-lg)',
            borderBottom: '1px solid var(--color-border)',
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'space-between',
          }}
        >
          <h2 style={{ fontSize: 'var(--font-size-lg)', margin: 0 }}>New Chat</h2>
          <button
            onClick={onClose}
            style={{
              background: 'none',
              border: 'none',
              cursor: 'pointer',
              color: 'var(--color-text-secondary)',
              fontSize: 'var(--font-size-lg)',
            }}
          >
            ✕
          </button>
        </div>

        <div style={{ padding: 'var(--space-lg)' }}>
          <label
            style={{
              display: 'block',
              fontSize: 'var(--font-size-sm)',
              color: 'var(--color-text-secondary)',
              marginBottom: 'var(--space-sm)',
            }}
          >
            Working Directory
          </label>
          <input
            type="text"
            value={workdir}
            onChange={e => { setWorkdir(e.target.value); setError(null); }}
            onKeyDown={handleKeyDown}
            placeholder="/path/to/your/project"
            autoFocus
            style={{
              width: '100%',
              background: 'var(--color-bg-secondary)',
              border: `1px solid ${error ? 'var(--color-error)' : 'var(--color-border)'}`,
              borderRadius: 'var(--radius-sm)',
              padding: 'var(--space-sm) var(--space-md)',
              color: 'var(--color-text-primary)',
              fontSize: 'var(--font-size-sm)',
              outline: 'none',
              boxSizing: 'border-box',
            }}
          />
          {error && (
            <div style={{
              color: 'var(--color-error)',
              fontSize: 'var(--font-size-sm)',
              marginTop: 'var(--space-sm)',
            }}>
              {error}
            </div>
          )}
        </div>

        <div
          style={{
            padding: 'var(--space-md) var(--space-lg)',
            borderTop: '1px solid var(--color-border)',
            display: 'flex',
            justifyContent: 'flex-end',
            gap: 'var(--space-sm)',
          }}
        >
          <button
            onClick={onClose}
            disabled={isCreating}
            style={{
              padding: 'var(--space-sm) var(--space-lg)',
              background: 'transparent',
              border: '1px solid var(--color-border)',
              borderRadius: 'var(--radius-sm)',
              color: 'var(--color-text-secondary)',
              cursor: 'pointer',
              fontSize: 'var(--font-size-sm)',
            }}
          >
            Cancel
          </button>
          <button
            onClick={handleCreate}
            disabled={isCreating}
            style={{
              padding: 'var(--space-sm) var(--space-lg)',
              background: 'var(--color-accent)',
              border: 'none',
              borderRadius: 'var(--radius-sm)',
              color: 'white',
              cursor: 'pointer',
              fontSize: 'var(--font-size-sm)',
              fontWeight: 600,
            }}
          >
            {isCreating ? 'Starting...' : 'Start Chat'}
          </button>
        </div>
      </div>
    </div>
  );
}

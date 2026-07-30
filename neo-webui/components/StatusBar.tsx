'use client';
import React from 'react';

interface StatusBarProps {
  model?: string;
  tokens?: number;
  connected?: boolean;
}

export function StatusBar({ model, tokens, connected }: StatusBarProps) {
  return (
    <div style={{
      height: 'var(--statusbar-height)',
      display: 'flex',
      alignItems: 'center',
      justifyContent: 'space-between',
      padding: '0 var(--space-md)',
      background: 'var(--color-bg-tertiary)',
      borderTop: '1px solid var(--color-border)',
      fontSize: 'var(--font-size-sm)',
      color: 'var(--color-text-tertiary)',
    }}>
      <div style={{ display: 'flex', gap: 'var(--space-lg)' }}>
        {model && <span>{model}</span>}
        {tokens !== undefined && <span>{tokens} tokens</span>}
      </div>
      <div>
        {connected !== undefined && (
          <span style={{
            display: 'inline-flex',
            alignItems: 'center',
            gap: '4px',
            color: connected ? 'var(--color-success)' : 'var(--color-error)',
          }}>
            <span style={{ width: 8, height: 8, borderRadius: '50%', background: 'currentColor', display: 'inline-block' }} />
            {connected ? 'Connected' : 'Disconnected'}
          </span>
        )}
      </div>
    </div>
  );
}

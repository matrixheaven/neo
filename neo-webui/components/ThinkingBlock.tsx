'use client';
import React, { useState } from 'react';

interface ThinkingBlockProps {
  text: string;
  collapsed: boolean;
}

export function ThinkingBlock({ text, collapsed: initialCollapsed }: ThinkingBlockProps) {
  const [collapsed, setCollapsed] = useState(initialCollapsed);

  if (!text) return null;

  return (
    <div style={{
      marginBottom: 'var(--space-md)',
      border: '1px solid var(--color-border)',
      borderRadius: 'var(--radius-md)',
      background: 'var(--color-thinking)',
      overflow: 'hidden',
    }}>
      <div
        onClick={() => setCollapsed(!collapsed)}
        style={{
          padding: 'var(--space-sm) var(--space-md)',
          cursor: 'pointer',
          fontSize: 'var(--font-size-sm)',
          color: 'var(--color-text-secondary)',
          display: 'flex',
          alignItems: 'center',
          gap: 'var(--space-sm)',
        }}
        onKeyDown={(e) => {
          if (e.key === 'Enter' || e.key === ' ') {
            e.preventDefault();
            setCollapsed(!collapsed);
          }
        }}
        role="button"
        tabIndex={0}
        aria-expanded={!collapsed}
      >
        <span>{collapsed ? '▶' : '▼'}</span>
        <span>🧠 Thinking</span>
      </div>
      {!collapsed && (
        <div style={{
          padding: 'var(--space-sm) var(--space-md) var(--space-md)',
          fontSize: 'var(--font-size-sm)',
          color: 'var(--color-text-secondary)',
          lineHeight: 1.5,
          whiteSpace: 'pre-wrap',
          wordBreak: 'break-word',
        }}>
          {text}
        </div>
      )}
    </div>
  );
}

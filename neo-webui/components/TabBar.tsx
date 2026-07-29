'use client';
import React from 'react';

export interface Tab {
  id: string;
  label: string;
  type: 'chat' | 'file';
  path?: string;
  sessionId?: string;
  workspace?: string;
}

interface TabBarProps {
  tabs: Tab[];
  activeTabId: string;
  onSelectTab: (id: string) => void;
  onCloseTab: (id: string) => void;
}

export function TabBar({ tabs, activeTabId, onSelectTab, onCloseTab }: TabBarProps) {
  return (
    <div style={{
      height: 'var(--tab-height)',
      display: 'flex',
      background: 'var(--color-bg-tertiary)',
      borderBottom: '1px solid var(--color-border)',
      overflow: 'hidden',
    }}>
      {tabs.map(tab => (
        <div
          key={tab.id}
          onClick={() => onSelectTab(tab.id)}
          style={{
            display: 'flex',
            alignItems: 'center',
            gap: 'var(--space-sm)',
            padding: '0 var(--space-md)',
            height: '100%',
            fontSize: 'var(--font-size-sm)',
            cursor: 'pointer',
            background: tab.id === activeTabId ? 'var(--color-bg-primary)' : 'transparent',
            borderRight: '1px solid var(--color-border)',
            color: tab.id === activeTabId ? 'var(--color-text-primary)' : 'var(--color-text-secondary)',
            whiteSpace: 'nowrap',
          }}
        >
          <span>{tab.type === 'chat' ? '💬' : '📄'}</span>
          <span>{tab.label}</span>
          <span
            onClick={(e) => { e.stopPropagation(); onCloseTab(tab.id); }}
            style={{ marginLeft: 'var(--space-xs)', opacity: 0.5, cursor: 'pointer' }}
          >
            ×
          </span>
        </div>
      ))}
    </div>
  );
}

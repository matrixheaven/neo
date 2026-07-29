'use client';
import React, { useState, useCallback } from 'react';
import { SessionSidebar } from './SessionSidebar';
import { TabBar, type Tab } from './TabBar';
import { StatusBar } from './StatusBar';
import { FileExplorer } from './FileExplorer';
import { useTheme } from '@/hooks/useTheme';

export function AppShell({ children }: { children: React.ReactNode }) {
  const { toggle } = useTheme();
  const [activeSessionId, setActiveSessionId] = useState<string | null>(null);
  const [showFileExplorer, setShowFileExplorer] = useState(false);
  const [tabs, setTabs] = useState<Tab[]>([
    { id: 'home', label: 'Home', type: 'chat' },
  ]);
  const [activeTabId, setActiveTabId] = useState('home');

  const handleSelectSession = useCallback((id: string) => {
    setActiveSessionId(id);
    const tabId = `chat-${id}`;
    const existing = tabs.find(t => t.id === tabId);
    if (!existing) {
      setTabs(prev => [...prev, { id: tabId, label: id.slice(0, 12), type: 'chat', sessionId: id }]);
    }
    setActiveTabId(tabId);
  }, [tabs]);

  const handleCloseTab = useCallback((id: string) => {
    setTabs(prev => {
      const next = prev.filter(t => t.id !== id);
      if (id === activeTabId && next.length > 0) {
        setActiveTabId(next[next.length - 1].id);
      }
      return next;
    });
  }, [activeTabId]);

  return (
    <div style={{ display: 'flex', flexDirection: 'column', height: '100vh', width: '100vw' }}>
      <div style={{ flex: 1, display: 'flex', overflow: 'hidden' }}>
        <SessionSidebar
          activeSessionId={activeSessionId}
          onSelectSession={handleSelectSession}
          onToggleFileExplorer={() => setShowFileExplorer(prev => !prev)}
          showFileExplorer={showFileExplorer}
        />

        {showFileExplorer && (
          <FileExplorer
            onOpenFile={(path) => {
              const tabId = `file-${path}`;
              const existing = tabs.find(t => t.id === tabId);
              if (!existing) {
                setTabs(prev => [...prev, { id: tabId, label: path.split('/').pop() || path, type: 'file', path }]);
              }
              setActiveTabId(tabId);
            }}
          />
        )}

        <div style={{ flex: 1, display: 'flex', flexDirection: 'column', overflow: 'hidden' }}>
          <TabBar
            tabs={tabs}
            activeTabId={activeTabId}
            onSelectTab={setActiveTabId}
            onCloseTab={handleCloseTab}
          />
          <div style={{ flex: 1, overflow: 'hidden' }}>
            {children}
          </div>
        </div>
      </div>
      <StatusBar connected={true} />
    </div>
  );
}

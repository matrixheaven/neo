'use client';
import React, { useState, useCallback } from 'react';
import { useRouter } from 'next/navigation';
import { SessionSidebar } from './SessionSidebar';
import { TabBar, type Tab } from './TabBar';
import { FileExplorer } from './FileExplorer';
import { ModelsConfig } from './ModelsConfig';
import { FileViewer } from './FileViewer';
import { useTheme } from '@/hooks/useTheme';

export function AppShell({ children }: { children: React.ReactNode }) {
  const router = useRouter();
  const { isDark, toggle } = useTheme();
  const [activeSessionId, setActiveSessionId] = useState<string | null>(null);
  const [activeSessionWorkdir, setActiveSessionWorkdir] = useState<string | null>(null);
  const [showFileExplorer, setShowFileExplorer] = useState(false);
  const [showConfig, setShowConfig] = useState(false);
  const [tabs, setTabs] = useState<Tab[]>([
    { id: 'home', label: 'Home', type: 'chat' },
  ]);
  const [activeTabId, setActiveTabId] = useState('home');

  const handleSelectSession = useCallback((id: string, workdir: string) => {
    setActiveSessionId(id);
    setActiveSessionWorkdir(workdir);
    const tabId = `chat-${id}`;
    const existing = tabs.find(t => t.id === tabId);
    if (!existing) {
      setTabs(prev => [...prev, { id: tabId, label: id.slice(0, 12), type: 'chat', sessionId: id }]);
    }
    setActiveTabId(tabId);
    router.push(`/session/${id}`);
  }, [tabs, router]);

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
            workspace={activeSessionWorkdir}
            onOpenFile={(path) => {
              const tabId = `file-${path}`;
              const existing = tabs.find(t => t.id === tabId);
              if (!existing) {
                setTabs(prev => [...prev, { id: tabId, label: path.split('/').pop() || path, type: 'file', path, workspace: activeSessionWorkdir ?? undefined }]);
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
            {(() => {
              const activeTab = tabs.find(t => t.id === activeTabId);
              if (activeTab?.type === 'file' && activeTab.path) {
                return <FileViewer filePath={activeTab.path} workspace={activeTab.workspace} />;
              }
              return children;
            })()}
          </div>
        </div>
      </div>
      <div style={{ display: 'flex', borderTop: '1px solid var(--color-border)' }}>
        <div style={{ flex: 1, display: 'flex', alignItems: 'center', padding: '0 var(--space-md)', height: 'var(--statusbar-height)', background: 'var(--color-bg-tertiary)', fontSize: 'var(--font-size-sm)', color: 'var(--color-text-tertiary)' }}>
          <span>Neo WebUI</span>
          <button onClick={toggle} style={{ marginLeft: 'var(--space-md)', background: 'none', border: 'none', cursor: 'pointer', fontSize: 'var(--font-size-sm)' }}>
            {isDark ? '☀️' : '🌙'}
          </button>
          <button onClick={() => setShowConfig(true)} style={{ marginLeft: 'var(--space-sm)', background: 'none', border: 'none', cursor: 'pointer', fontSize: 'var(--font-size-sm)' }}>
            ⚙️
          </button>
        </div>
        <div style={{ padding: '0 var(--space-md)', display: 'flex', alignItems: 'center', height: 'var(--statusbar-height)', background: 'var(--color-bg-tertiary)', fontSize: 'var(--font-size-sm)', color: 'var(--color-text-tertiary)' }}>
          <span style={{ display: 'inline-flex', alignItems: 'center', gap: '4px' }}>
            <span style={{ width: 6, height: 6, borderRadius: '50%', background: 'var(--color-success)', display: 'inline-block' }} />
            Ready
          </span>
        </div>
      </div>
      {showConfig && <ModelsConfig onClose={() => setShowConfig(false)} />}
    </div>
  );
}

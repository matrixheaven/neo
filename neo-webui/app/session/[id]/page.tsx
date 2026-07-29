'use client';
import React, { use } from 'react';
import { AppShell } from '@/components/AppShell';
import { ChatWindow } from '@/components/ChatWindow';
import { ChatInput } from '@/components/ChatInput';
import { PermissionDialog } from '@/components/PermissionDialog';
import { useAgentSession } from '@/hooks/useAgentSession';

export default function SessionPage({ params }: { params: Promise<{ id: string }> }) {
  const { id } = use(params);
  const { messages, isStreaming, error, sendMessage, pendingApproval, approveTool, rejectTool } = useAgentSession(id);

  return (
    <AppShell>
      <div style={{ display: 'flex', flexDirection: 'column', height: '100%' }}>
        <ChatWindow messages={messages} isStreaming={isStreaming} />
        {error && (
          <div style={{
            padding: 'var(--space-sm) var(--space-md)',
            background: 'var(--color-diff-removed)',
            color: 'var(--color-error)',
            fontSize: 'var(--font-size-sm)',
          }}>
            {error}
          </div>
        )}
        <ChatInput onSend={sendMessage} disabled={isStreaming || !!pendingApproval} />
      </div>
      {pendingApproval && (
        <PermissionDialog
          request={pendingApproval}
          onApprove={approveTool}
          onReject={rejectTool}
        />
      )}
    </AppShell>
  );
}

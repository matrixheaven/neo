'use client';
import React, { useState } from 'react';
import { useRouter } from 'next/navigation';
import { AppShell } from '@/components/AppShell';
import { ChatInput } from '@/components/ChatInput';

export default function HomePage() {
  const router = useRouter();
  const [isStarting, setIsStarting] = useState(false);

  const handleStartChat = async (message: string) => {
    setIsStarting(true);
    const sessionId = `new-${Date.now()}`;
    await fetch(`/api/sessions/${sessionId}/prompt`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ message }),
    });
    router.push(`/session/${sessionId}`);
  };

  return (
    <AppShell>
      <div style={{ display: 'flex', flexDirection: 'column', height: '100%' }}>
        <div style={{
          flex: 1,
          display: 'flex',
          flexDirection: 'column',
          alignItems: 'center',
          justifyContent: 'center',
          gap: 'var(--space-lg)',
          padding: 'var(--space-xl)',
        }}>
          <h1 style={{ fontSize: '28px', color: 'var(--color-text-primary)', fontWeight: 700 }}>Neo WebUI</h1>
          <p style={{ color: 'var(--color-text-tertiary)', fontSize: 'var(--font-size-lg)' }}>
            Your browser interface for Neo AI coding agent
          </p>
        </div>
        <ChatInput onSend={handleStartChat} disabled={isStarting} />
      </div>
    </AppShell>
  );
}

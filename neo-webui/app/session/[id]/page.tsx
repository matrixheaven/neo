'use client';
import React, { use } from 'react';
import { AppShell } from '@/components/AppShell';

export default function SessionPage({ params }: { params: Promise<{ id: string }> }) {
  const { id } = use(params);

  return (
    <AppShell>
      <div style={{
        display: 'flex',
        flexDirection: 'column',
        alignItems: 'center',
        justifyContent: 'center',
        height: '100%',
        color: 'var(--color-text-tertiary)',
      }}>
        <p>Session: {id}</p>
        <p style={{ marginTop: 'var(--space-md)' }}>Chat interface coming soon...</p>
      </div>
    </AppShell>
  );
}

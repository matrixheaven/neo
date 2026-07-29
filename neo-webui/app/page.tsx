'use client';
import React from 'react';
import { AppShell } from '@/components/AppShell';

export default function HomePage() {
  return (
    <AppShell>
      <div style={{
        display: 'flex',
        flexDirection: 'column',
        alignItems: 'center',
        justifyContent: 'center',
        height: '100%',
        color: 'var(--color-text-tertiary)',
        gap: 'var(--space-lg)',
      }}>
        <h1 style={{ fontSize: 'var(--font-size-xl)', color: 'var(--color-text-secondary)' }}>
          Neo WebUI
        </h1>
        <p>Select a session from the sidebar to start chatting</p>
      </div>
    </AppShell>
  );
}

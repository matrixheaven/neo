'use client';
import React, { useRef, useEffect } from 'react';
import { MessageView } from './MessageView';
import type { Message } from '@/hooks/useAgentSession';

interface ChatWindowProps {
  messages: Message[];
  isStreaming: boolean;
}

export function ChatWindow({ messages, isStreaming }: ChatWindowProps) {
  const bottomRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [messages]);

  if (messages.length === 0) {
    return (
      <div style={{
        display: 'flex',
        flexDirection: 'column',
        alignItems: 'center',
        justifyContent: 'center',
        height: '100%',
        color: 'var(--color-text-tertiary)',
        gap: 'var(--space-md)',
      }}>
        <p style={{ fontSize: 'var(--font-size-lg)' }}>Start a conversation</p>
        <p style={{ fontSize: 'var(--font-size-sm)' }}>Type a message below to begin</p>
      </div>
    );
  }

  return (
    <div style={{
      flex: 1,
      overflowY: 'auto',
      padding: 'var(--space-md) 0',
    }}>
      {messages.map((msg, i) => (
        <MessageView key={msg.id || i} message={msg} />
      ))}
      {isStreaming && (
        <div style={{ padding: 'var(--space-md) var(--space-xl)', color: 'var(--color-text-tertiary)', fontSize: 'var(--font-size-sm)' }}>
          <span className="pulse">●</span> Streaming...
        </div>
      )}
      <div ref={bottomRef} />
    </div>
  );
}

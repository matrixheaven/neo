'use client';
import React from 'react';
import { MarkdownBody } from './MarkdownBody';
import { ThinkingBlock } from './ThinkingBlock';
import { ToolCallCard } from './ToolCallCard';
import type { Message } from '@/hooks/useAgentSession';

export function MessageView({ message }: { message: Message }) {
  const isUser = message.role === 'user';

  return (
    <div style={{
      padding: 'var(--space-md) var(--space-xl)',
      borderBottom: '1px solid var(--color-border-light)',
      background: isUser ? 'transparent' : 'var(--color-bg-secondary)',
    }}>
      <div style={{
        fontSize: 'var(--font-size-xs, 11px)',
        color: 'var(--color-text-tertiary)',
        marginBottom: 'var(--space-sm)',
        fontWeight: 600,
        textTransform: 'uppercase',
      }}>
        {isUser ? 'You' : 'Neo'}
      </div>

      {message.thinking && (
        <ThinkingBlock text={message.thinking.text} collapsed={message.thinking.collapsed} />
      )}

      {message.content && (
        <div style={{ fontSize: 'var(--font-size-base)', lineHeight: 1.6 }}>
          <MarkdownBody text={message.content} />
        </div>
      )}

      {message.toolCalls?.map(tc => (
        <ToolCallCard key={tc.id} toolCall={tc} />
      ))}
    </div>
  );
}

'use client';
import React, { useState } from 'react';
import type { ToolCallInfo } from '@/hooks/useAgentSession';

interface ToolCallCardProps {
  toolCall: ToolCallInfo;
}

export function ToolCallCard({ toolCall }: ToolCallCardProps) {
  const [argsExpanded, setArgsExpanded] = useState(false);
  const [resultExpanded, setResultExpanded] = useState(true);

  const statusIcon = toolCall.status === 'done' ? '✅' : toolCall.status === 'running' ? '⏳' : '🕐';

  let parsedArgs: Record<string, unknown> = {};
  try {
    parsedArgs = JSON.parse(toolCall.arguments || '{}') as Record<string, unknown>;
  } catch {
    // keep empty object
  }

  return (
    <div style={{
      margin: 'var(--space-sm) 0',
      border: '1px solid var(--color-border)',
      borderRadius: 'var(--radius-md)',
      background: toolCall.result?.isError ? 'var(--color-diff-removed)' : 'var(--color-tool-call)',
      overflow: 'hidden',
    }}>
      <div style={{
        padding: 'var(--space-sm) var(--space-md)',
        fontSize: 'var(--font-size-sm)',
        display: 'flex',
        alignItems: 'center',
        gap: 'var(--space-sm)',
      }}>
        <span>{statusIcon}</span>
        <span style={{ fontWeight: 600, color: 'var(--color-text-primary)' }}>{toolCall.name}</span>
        <span
          onClick={() => setArgsExpanded(!argsExpanded)}
          onKeyDown={(e) => {
            if (e.key === 'Enter' || e.key === ' ') {
              e.preventDefault();
              setArgsExpanded(!argsExpanded);
            }
          }}
          role="button"
          tabIndex={0}
          style={{ cursor: 'pointer', color: 'var(--color-text-tertiary)', fontSize: 'var(--font-size-xs, 11px)' }}
        >
          {argsExpanded ? 'Hide args' : 'Show args'}
        </span>
      </div>

      {argsExpanded && (
        <pre style={{
          padding: 'var(--space-sm) var(--space-md)',
          fontSize: 'var(--font-size-xs, 11px)',
          color: 'var(--color-text-secondary)',
          whiteSpace: 'pre-wrap',
          wordBreak: 'break-word',
          borderTop: '1px solid var(--color-border-light)',
          margin: 0,
        }}>
          {JSON.stringify(parsedArgs, null, 2)}
        </pre>
      )}

      {toolCall.result && (
        <div style={{ borderTop: '1px solid var(--color-border-light)' }}>
          <div
            onClick={() => setResultExpanded(!resultExpanded)}
            onKeyDown={(e) => {
              if (e.key === 'Enter' || e.key === ' ') {
                e.preventDefault();
                setResultExpanded(!resultExpanded);
              }
            }}
            role="button"
            tabIndex={0}
            style={{
              padding: 'var(--space-xs) var(--space-md)',
              cursor: 'pointer',
              fontSize: 'var(--font-size-xs, 11px)',
              color: 'var(--color-text-tertiary)',
            }}
          >
            {resultExpanded ? '▼' : '▶'} Result
          </div>
          {resultExpanded && (
            <pre style={{
              padding: 'var(--space-sm) var(--space-md) var(--space-md)',
              fontSize: 'var(--font-size-xs, 11px)',
              color: 'var(--color-text-secondary)',
              whiteSpace: 'pre-wrap',
              wordBreak: 'break-word',
              maxHeight: 300,
              overflowY: 'auto',
              margin: 0,
            }}>
              {toolCall.result.content}
            </pre>
          )}
        </div>
      )}
    </div>
  );
}

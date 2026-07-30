'use client';
import React from 'react';
import type { ApprovalRequest } from '@/lib/types';

interface PermissionDialogProps {
  request: ApprovalRequest;
  onApprove: (action: string) => void;
  onReject: () => void;
}

export function PermissionDialog({
  request,
  onApprove,
  onReject,
}: PermissionDialogProps) {
  const { presentation } = request;

  const renderPresentation = () => {
    switch (presentation.kind) {
      case 'command':
        return (
          <div>
            <div style={{ fontWeight: 600, marginBottom: 'var(--space-sm)' }}>Command</div>
            <pre
              style={{
                background: 'var(--color-bg-tertiary)',
                borderRadius: 'var(--radius-md)',
                padding: 'var(--space-md)',
                fontSize: 'var(--font-size-sm)',
                fontFamily: 'var(--font-mono)',
                whiteSpace: 'pre-wrap',
              }}
            >
              {presentation.command}
            </pre>
            {presentation.cwd && (
              <div
                style={{
                  marginTop: 'var(--space-sm)',
                  fontSize: 'var(--font-size-sm)',
                  color: 'var(--color-text-tertiary)',
                }}
              >
                Directory: {presentation.cwd}
              </div>
            )}
          </div>
        );
      case 'tool':
        return (
          <div>
            <div style={{ fontWeight: 600, marginBottom: 'var(--space-sm)' }}>Tool</div>
            {presentation.details.map((d, i) => (
              <div
                key={i}
                style={{ fontSize: 'var(--font-size-sm)', color: 'var(--color-text-secondary)' }}
              >
                {d}
              </div>
            ))}
          </div>
        );
      case 'edit':
        return (
          <div>
            <div style={{ fontWeight: 600, marginBottom: 'var(--space-sm)' }}>Edit files</div>
            <pre
              style={{
                background: 'var(--color-bg-tertiary)',
                borderRadius: 'var(--radius-md)',
                padding: 'var(--space-md)',
                fontSize: 'var(--font-size-sm)',
                fontFamily: 'var(--font-mono)',
                whiteSpace: 'pre-wrap',
              }}
            >
              {JSON.stringify(presentation.edit, null, 2)}
            </pre>
          </div>
        );
      case 'write':
        return (
          <div>
            <div style={{ fontWeight: 600, marginBottom: 'var(--space-sm)' }}>Write files</div>
            <pre
              style={{
                background: 'var(--color-bg-tertiary)',
                borderRadius: 'var(--radius-md)',
                padding: 'var(--space-md)',
                fontSize: 'var(--font-size-sm)',
                fontFamily: 'var(--font-mono)',
                whiteSpace: 'pre-wrap',
              }}
            >
              {JSON.stringify(presentation.write, null, 2)}
            </pre>
          </div>
        );
      case 'plan':
        return (
          <div>
            <div style={{ fontWeight: 600, marginBottom: 'var(--space-sm)' }}>Plan</div>
            {presentation.summary && (
              <div style={{ fontSize: 'var(--font-size-sm)', color: 'var(--color-text-secondary)', marginBottom: 'var(--space-sm)' }}>
                {presentation.summary}
              </div>
            )}
            <pre
              style={{
                background: 'var(--color-bg-tertiary)',
                borderRadius: 'var(--radius-md)',
                padding: 'var(--space-md)',
                fontSize: 'var(--font-size-sm)',
                fontFamily: 'var(--font-mono)',
                whiteSpace: 'pre-wrap',
                maxHeight: 300,
                overflow: 'auto',
              }}
            >
              {presentation.markdown}
            </pre>
          </div>
        );
      case 'goal':
        return (
          <div>
            <div style={{ fontWeight: 600, marginBottom: 'var(--space-sm)' }}>Goal</div>
            <div style={{ fontSize: 'var(--font-size-base)', marginBottom: 'var(--space-sm)' }}>
              {presentation.objective}
            </div>
            {presentation.completion_criterion && (
              <div style={{ fontSize: 'var(--font-size-sm)', color: 'var(--color-text-secondary)', marginBottom: 'var(--space-sm)' }}>
                Completion: {presentation.completion_criterion}
              </div>
            )}
            {presentation.phases.length > 0 && (
              <div>
                <div style={{ fontWeight: 600, fontSize: 'var(--font-size-sm)', marginBottom: 'var(--space-xs)' }}>
                  Phases:
                </div>
                {presentation.phases.map((p, i) => (
                  <div
                    key={i}
                    style={{ fontSize: 'var(--font-size-sm)', color: 'var(--color-text-secondary)' }}
                  >
                    {i + 1}. {p}
                  </div>
                ))}
              </div>
            )}
          </div>
        );
      case 'workflow':
        return (
          <div>
            <div style={{ fontWeight: 600, marginBottom: 'var(--space-sm)' }}>Workflow</div>
            <pre
              style={{
                background: 'var(--color-bg-tertiary)',
                borderRadius: 'var(--radius-md)',
                padding: 'var(--space-md)',
                fontSize: 'var(--font-size-sm)',
                fontFamily: 'var(--font-mono)',
                whiteSpace: 'pre-wrap',
              }}
            >
              {JSON.stringify(presentation.workflow, null, 2)}
            </pre>
          </div>
        );
    }
  };

  return (
    <div
      style={{
        position: 'fixed',
        inset: 0,
        zIndex: 200,
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
      }}
    >
      <div style={{ position: 'absolute', inset: 0, background: 'rgba(0,0,0,0.5)' }} />
      <div
        style={{
          position: 'relative',
          width: 480,
          maxHeight: '80vh',
          background: 'var(--color-bg-primary)',
          borderRadius: 'var(--radius-lg)',
          boxShadow: 'var(--shadow-lg)',
          overflow: 'hidden',
          display: 'flex',
          flexDirection: 'column',
        }}
      >
        <div
          style={{
            padding: 'var(--space-md) var(--space-lg)',
            borderBottom: '1px solid var(--color-border)',
            background: 'var(--color-accent-subtle)',
          }}
        >
          <div
            style={{
              fontSize: 'var(--font-size-xs, 11px)',
              color: 'var(--color-text-tertiary)',
              textTransform: 'uppercase',
              marginBottom: 4,
            }}
          >
            Permission Required — {request.operation}
          </div>
          <h2 style={{ fontSize: 'var(--font-size-lg)', margin: 0 }}>
            {presentation.title}
          </h2>
        </div>

        <div style={{ flex: 1, overflow: 'auto', padding: 'var(--space-lg)' }}>
          {renderPresentation()}
        </div>

        <div
          style={{
            padding: 'var(--space-md) var(--space-lg)',
            borderTop: '1px solid var(--color-border)',
            display: 'flex',
            gap: 'var(--space-md)',
            justifyContent: 'flex-end',
          }}
        >
          <button
            onClick={onReject}
            style={{
              padding: 'var(--space-sm) var(--space-xl)',
              border: '1px solid var(--color-border)',
              borderRadius: 'var(--radius-md)',
              background: 'transparent',
              color: 'var(--color-text-secondary)',
              cursor: 'pointer',
              fontSize: 'var(--font-size-base)',
            }}
          >
            Reject
          </button>
          <button
            onClick={() => onApprove('permit_once')}
            style={{
              padding: 'var(--space-sm) var(--space-xl)',
              border: 'none',
              borderRadius: 'var(--radius-md)',
              background: 'var(--color-accent)',
              color: 'white',
              cursor: 'pointer',
              fontSize: 'var(--font-size-base)',
              fontWeight: 600,
            }}
          >
            Approve
          </button>
        </div>
      </div>
    </div>
  );
}

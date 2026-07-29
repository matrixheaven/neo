'use client';
import React, { useEffect, useState } from 'react';

interface FileViewerProps {
  filePath: string;
}

export function FileViewer({ filePath }: FileViewerProps) {
  const [content, setContent] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setLoading(true);
    setError(null);
    const segments = filePath
      .split('/')
      .map((s) => encodeURIComponent(s))
      .join('/');
    fetch(`/api/files/${segments}`)
      .then((r) => r.json())
      .then((data: { content?: string; error?: string }) => {
        if (data.error) setError(data.error);
        else setContent(data.content ?? null);
      })
      .catch((e: Error) => setError(e.message))
      .finally(() => setLoading(false));
  }, [filePath]);

  const extension = filePath.split('.').pop()?.toLowerCase();
  const getLanguage = (): string => {
    const map: Record<string, string> = {
      ts: 'typescript',
      tsx: 'typescript',
      js: 'javascript',
      jsx: 'javascript',
      rs: 'rust',
      json: 'json',
      md: 'markdown',
      css: 'css',
      html: 'html',
      toml: 'toml',
      yaml: 'yaml',
      yml: 'yaml',
    };
    return map[extension || ''] || 'plaintext';
  };

  return (
    <div style={{ height: '100%', display: 'flex', flexDirection: 'column' }}>
      <div
        style={{
          padding: 'var(--space-sm) var(--space-md)',
          background: 'var(--color-bg-tertiary)',
          borderBottom: '1px solid var(--color-border)',
          fontSize: 'var(--font-size-sm)',
          color: 'var(--color-text-secondary)',
          display: 'flex',
          alignItems: 'center',
          gap: 'var(--space-sm)',
        }}
      >
        <span>📄</span>
        <span>{filePath}</span>
        <span style={{ color: 'var(--color-text-tertiary)', marginLeft: 'auto' }}>
          {getLanguage()}
        </span>
      </div>
      <div style={{ flex: 1, overflow: 'auto' }}>
        {loading ? (
          <div
            style={{
              padding: 'var(--space-xl)',
              color: 'var(--color-text-tertiary)',
              textAlign: 'center',
            }}
          >
            Loading...
          </div>
        ) : error ? (
          <div style={{ padding: 'var(--space-xl)', color: 'var(--color-error)' }}>
            {error}
          </div>
        ) : (
          <pre
            style={{
              margin: 0,
              padding: 'var(--space-md)',
              fontSize: 'var(--font-size-sm)',
              fontFamily: 'var(--font-mono)',
              lineHeight: 1.5,
              color: 'var(--color-text-primary)',
              whiteSpace: 'pre-wrap',
              wordBreak: 'break-word',
              tabSize: 2,
            }}
          >
            <code>{content}</code>
          </pre>
        )}
      </div>
    </div>
  );
}

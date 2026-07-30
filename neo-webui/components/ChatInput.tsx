'use client';
import React, { useState, useRef, useEffect } from 'react';

interface ChatInputProps {
  onSend: (text: string) => void;
  disabled?: boolean;
}

export function ChatInput({ onSend, disabled }: ChatInputProps) {
  const [text, setText] = useState('');
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  const autoResize = () => {
    const el = textareaRef.current;
    if (!el) return;
    el.style.height = 'auto';
    el.style.height = `${Math.min(el.scrollHeight, 200)}px`;
  };

  useEffect(() => {
    autoResize();
  }, [text]);

  const handleSend = () => {
    const trimmed = text.trim();
    if (!trimmed || disabled) return;
    onSend(trimmed);
    setText('');
  };

  return (
    <div style={{
      padding: 'var(--space-md)',
      borderTop: '1px solid var(--color-border)',
      background: 'var(--color-bg-primary)',
      display: 'flex',
      gap: 'var(--space-md)',
    }}>
      <textarea
        ref={textareaRef}
        value={text}
        onChange={e => setText(e.target.value)}
        onKeyDown={e => {
          if (e.key === 'Enter' && !e.shiftKey) {
            e.preventDefault();
            handleSend();
          }
        }}
        placeholder="Type a message... (Enter to send, Shift+Enter for newline)"
        disabled={disabled}
        style={{
          flex: 1,
          background: 'var(--color-bg-secondary)',
          border: '1px solid var(--color-border)',
          borderRadius: 'var(--radius-md)',
          padding: 'var(--space-md)',
          color: 'var(--color-text-primary)',
          fontSize: 'var(--font-size-base)',
          resize: 'none',
          outline: 'none',
          minHeight: 44,
          maxHeight: 200,
          fontFamily: 'inherit',
        }}
        rows={1}
      />
      <button
        onClick={handleSend}
        disabled={disabled || !text.trim()}
        style={{
          background: disabled || !text.trim() ? 'var(--color-bg-tertiary)' : 'var(--color-accent)',
          color: disabled || !text.trim() ? 'var(--color-text-tertiary)' : 'white',
          border: 'none',
          borderRadius: 'var(--radius-md)',
          padding: '0 var(--space-xl)',
          cursor: disabled || !text.trim() ? 'default' : 'pointer',
          fontSize: 'var(--font-size-base)',
          fontWeight: 600,
        }}
      >
        Send
      </button>
    </div>
  );
}

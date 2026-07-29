'use client';
import React from 'react';

function escapeHtml(text: string): string {
  return text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}

function renderMarkdown(text: string): string {
  let html = escapeHtml(text);
  // Code blocks ```
  html = html.replace(/```(\w*)\n([\s\S]*?)```/g, (_, lang, code) =>
    `<pre style="background:var(--color-bg-tertiary);border-radius:var(--radius-md);padding:var(--space-md);overflow-x:auto;margin:var(--space-sm) 0;"><code>${escapeHtml(code.trim())}</code></pre>`
  );
  // Inline code `...`
  html = html.replace(/`([^`]+)`/g, '<code style="background:var(--color-bg-tertiary);padding:1px 4px;border-radius:3px;font-size:0.9em;">$1</code>');
  // Bold **...**
  html = html.replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>');
  // Italic *...*
  html = html.replace(/\*([^*]+)\*/g, '<em>$1</em>');
  // Newlines to <br>
  html = html.replace(/\n/g, '<br>');
  return html;
}

export function MarkdownBody({ text }: { text: string }) {
  return <div dangerouslySetInnerHTML={{ __html: renderMarkdown(text) }} />;
}

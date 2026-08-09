/**
 * Safe Markdown rendering: react-markdown with raw HTML disabled (default),
 * no dangerouslySetInnerHTML anywhere, and links restricted to http/https.
 */

import { memo } from "react";
import ReactMarkdown from "react-markdown";
import type { Components } from "react-markdown";

function safeUrl(url: string | undefined): string | undefined {
  if (!url) return undefined;
  const trimmed = url.trim().toLowerCase();
  if (trimmed.startsWith("http://") || trimmed.startsWith("https://")) {
    return url;
  }
  return undefined;
}

const components: Components = {
  a({ href, children }) {
    const safe = safeUrl(typeof href === "string" ? href : undefined);
    if (!safe) {
      return <span>{children}</span>;
    }
    return (
      <a href={safe} target="_blank" rel="noreferrer noopener">
        {children}
      </a>
    );
  },
  img() {
    // Remote images are outside the local-only surface.
    return null;
  },
};

export const Markdown = memo(function Markdown({ text }: { text: string }) {
  return (
    <div className="markdown">
      <ReactMarkdown components={components} skipHtml>
        {text}
      </ReactMarkdown>
    </div>
  );
});

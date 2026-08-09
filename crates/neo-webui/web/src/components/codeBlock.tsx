/**
 * Code block with an icon copy button. Code and output are untrusted text:
 * rendered via text nodes inside <pre><code>, never as HTML.
 */

import { Check, Copy } from "lucide-react";
import { useEffect, useRef, useState, type ReactNode } from "react";

export function CopyButton({ text, label }: { text: string; label: string }) {
  const [copied, setCopied] = useState(false);
  const timerRef = useRef<number | null>(null);

  useEffect(
    () => () => {
      if (timerRef.current !== null) {
        window.clearTimeout(timerRef.current);
      }
    },
    [],
  );

  return (
    <button
      type="button"
      className="icon-button copy-button"
      aria-label={copied ? "已复制" : label}
      title={copied ? "已复制" : label}
      onClick={() => {
        void navigator.clipboard
          .writeText(text)
          .then(() => {
            setCopied(true);
            if (timerRef.current !== null) {
              window.clearTimeout(timerRef.current);
            }
            timerRef.current = window.setTimeout(() => setCopied(false), 1500);
          })
          .catch(() => {});
      }}
    >
      {copied ? <Check size={14} aria-hidden /> : <Copy size={14} aria-hidden />}
    </button>
  );
}

export function CodeBlock({
  code,
  language,
}: {
  code: string;
  language?: string;
}) {
  return (
    <div className="code-block">
      <div className="code-block-bar">
        <span className="code-block-lang">{language ?? "text"}</span>
        <CopyButton text={code} label="复制代码" />
      </div>
      <pre>
        <code>{code}</code>
      </pre>
    </div>
  );
}

export function OutputBlock({ text, children }: { text: string; children?: ReactNode }) {
  return (
    <div className="output-block">
      <div className="code-block-bar">
        <span className="code-block-lang">输出</span>
        <CopyButton text={text} label="复制输出" />
        {children}
      </div>
      <pre>
        <code>{text}</code>
      </pre>
    </div>
  );
}

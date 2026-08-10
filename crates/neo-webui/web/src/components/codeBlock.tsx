/**
 * Code block with an icon copy button. Code and output are untrusted text:
 * Prism tokens are rendered as React text nodes, never as HTML.
 */

import { Check, Copy } from "lucide-react";
import * as Prism from "prismjs";
import "prismjs/components/prism-bash";
import "prismjs/components/prism-diff";
import "prismjs/components/prism-docker";
import "prismjs/components/prism-ini";
import "prismjs/components/prism-jsx";
import "prismjs/components/prism-json";
import "prismjs/components/prism-markdown";
import "prismjs/components/prism-python";
import "prismjs/components/prism-rust";
import "prismjs/components/prism-sql";
import "prismjs/components/prism-toml";
import "prismjs/components/prism-typescript";
import "prismjs/components/prism-tsx";
import "prismjs/components/prism-yaml";
import { useEffect, useRef, useState, type CSSProperties, type ReactNode } from "react";

const languageAliases: Readonly<Record<string, string>> = {
  "参数": "json",
  bash: "bash",
  cjs: "javascript",
  cfg: "ini",
  conf: "ini",
  css: "css",
  diff: "diff",
  docker: "docker",
  dockerfile: "docker",
  htm: "markup",
  html: "markup",
  ini: "ini",
  javascript: "javascript",
  js: "javascript",
  jsx: "jsx",
  json: "json",
  jsonc: "json",
  markdown: "markdown",
  md: "markdown",
  mjs: "javascript",
  patch: "diff",
  py: "python",
  python: "python",
  rs: "rust",
  rust: "rust",
  sh: "bash",
  shell: "bash",
  sql: "sql",
  svg: "markup",
  toml: "toml",
  ts: "typescript",
  tsx: "tsx",
  typescript: "typescript",
  xml: "markup",
  yaml: "yaml",
  yml: "yaml",
  zsh: "bash",
};

const tokenColors: Readonly<Record<string, string>> = {
  "attr-name": "var(--ok)",
  "attr-value": "var(--ok)",
  boolean: "var(--accent)",
  builtin: "var(--accent)",
  "class-name": "var(--accent)",
  comment: "var(--text-faint)",
  deleted: "var(--danger)",
  function: "var(--accent)",
  important: "var(--danger)",
  keyword: "var(--accent)",
  number: "var(--accent)",
  operator: "var(--text-dim)",
  property: "var(--ok)",
  punctuation: "var(--text-dim)",
  regex: "var(--danger)",
  selector: "var(--ok)",
  string: "var(--ok)",
  tag: "var(--danger)",
};

function syntaxLanguage(language: string | undefined): string | null {
  if (!language) return null;
  const normalized = language.trim().toLowerCase().replace(/^language-/, "");
  const direct = languageAliases[normalized];
  if (direct) return direct;

  const parts = normalized.split(/[\\/]/);
  const filename = parts[parts.length - 1];
  const filenameLanguage = languageAliases[filename];
  if (filenameLanguage) return filenameLanguage;

  const extension = filename.slice(filename.lastIndexOf(".") + 1);
  return languageAliases[extension] ?? null;
}

function tokenStyle(type: string): CSSProperties | undefined {
  const color = tokenColors[type];
  return color ? { color } : undefined;
}

function renderTokenStream(stream: Prism.TokenStream, key: string): ReactNode {
  if (typeof stream === "string") return stream;
  if (Array.isArray(stream)) {
    return stream.map((token, index) => renderTokenStream(token, `${key}-${index}`));
  }

  const aliases = Array.isArray(stream.alias) ? stream.alias : [stream.alias];
  return (
    <span
      key={key}
      className={["token", stream.type, ...aliases].join(" ")}
      style={tokenStyle(stream.type)}
    >
      {renderTokenStream(stream.content, `${key}-content`)}
    </span>
  );
}

function highlightedCode(code: string, language: string | undefined): { content: ReactNode; language: string | null } {
  const syntax = syntaxLanguage(language);
  const grammar = syntax === null ? undefined : Prism.languages[syntax];
  if (!grammar || syntax === null) return { content: code, language: null };
  return { content: renderTokenStream(Prism.tokenize(code, grammar), "syntax"), language: syntax };
}

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
  const highlighted = highlightedCode(code, language);
  return (
    <div className="code-block">
      <div className="code-block-bar">
        <span className="code-block-lang">{language ?? "text"}</span>
        <CopyButton text={code} label="复制代码" />
      </div>
      <pre>
        <code className={highlighted.language ? `language-${highlighted.language}` : undefined}>
          {highlighted.content}
        </code>
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

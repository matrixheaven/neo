import {
  AlignJustify,
  ChevronDown,
  ChevronRight,
  ChevronsDownUp,
  ChevronsUpDown,
  Columns2,
  Copy,
  FileCode2,
  FileSearch,
  FileText,
  FolderOpen,
  MoreHorizontal,
  RefreshCw,
  Search,
  WrapText,
} from "lucide-react";
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import type { ReviewTarget } from "../state/appState";
import type { TranscriptItem } from "../state/transcript";
import {
  reviewFilesForMessage,
  type FilePreviewLine,
  type ReviewFileChange,
} from "./transcript";
import { Markdown } from "./markdown";

export type ReviewSourceState = "ok" | "loading" | "missing" | "error";

interface ReviewPanelProps {
  target: ReviewTarget | null;
  items: TranscriptItem[] | null;
  sourceState: ReviewSourceState;
  refreshKey: number;
  onRefresh: () => void;
}

type DiffLayout = "unified" | "split";

function reviewFileId(path: string): string {
  return `review-file-${encodeURIComponent(path)}`;
}

function terminalState(state: ReviewSourceState): boolean {
  return state === "missing" || state === "error";
}

function compactLines(lines: FilePreviewLine[], full: boolean): FilePreviewLine[] {
  if (full) return lines;
  const output: FilePreviewLine[] = [];
  for (let index = 0; index < lines.length;) {
    if (lines[index]?.kind !== "context") {
      output.push(lines[index]);
      index += 1;
      continue;
    }
    let end = index;
    while (end < lines.length && lines[end]?.kind === "context") end += 1;
    const run = lines.slice(index, end);
    if (run.length <= 6) {
      output.push(...run);
    } else {
      output.push(
        ...run.slice(0, 2),
        { content: `${run.length - 4} 个未修改行`, kind: "separator" },
        ...run.slice(-2),
      );
    }
    index = end;
  }
  return output;
}

function whitespaceOnlyPairs(lines: FilePreviewLine[]): Set<number> {
  const hidden = new Set<number>();
  for (let index = 0; index + 1 < lines.length; index += 1) {
    const left = lines[index];
    const right = lines[index + 1];
    if (left.kind !== "del" || right.kind !== "add") continue;
    const normalizedLeft = left.content.slice(1).replace(/\s/g, "");
    const normalizedRight = right.content.slice(1).replace(/\s/g, "");
    if (normalizedLeft === normalizedRight) {
      hidden.add(index);
      hidden.add(index + 1);
    }
  }
  return hidden;
}

function wordRanges(lines: FilePreviewLine[]): Map<number, [number, number]> {
  const ranges = new Map<number, [number, number]>();
  for (let index = 0; index + 1 < lines.length; index += 1) {
    const removed = lines[index];
    const added = lines[index + 1];
    if (removed.kind !== "del" || added.kind !== "add") continue;
    const before = removed.content.slice(1);
    const after = added.content.slice(1);
    let prefix = 0;
    while (prefix < before.length && prefix < after.length && before[prefix] === after[prefix]) {
      prefix += 1;
    }
    let suffix = 0;
    while (
      suffix < before.length - prefix &&
      suffix < after.length - prefix &&
      before[before.length - 1 - suffix] === after[after.length - 1 - suffix]
    ) {
      suffix += 1;
    }
    ranges.set(index, [prefix + 1, removed.content.length - suffix]);
    ranges.set(index + 1, [prefix + 1, added.content.length - suffix]);
  }
  return ranges;
}

function DiffContent({
  line,
  range,
}: {
  line: FilePreviewLine;
  range: [number, number] | undefined;
}) {
  if (!range || range[0] >= range[1]) return <>{line.content || " "}</>;
  return (
    <>
      {line.content.slice(0, range[0])}
      <mark>{line.content.slice(range[0], range[1])}</mark>
      {line.content.slice(range[1])}
    </>
  );
}

function UnifiedDiff({ lines, wordDiff, wrap }: {
  lines: FilePreviewLine[];
  wordDiff: boolean;
  wrap: boolean;
}) {
  const ranges = wordDiff ? wordRanges(lines) : new Map<number, [number, number]>();
  return (
    <div className={`review-unified${wrap ? " wrap" : ""}`} role="table" aria-label="统一差异">
      {lines.map((line, index) => (
        <div className={`review-diff-line ft-diff-${line.kind}`} role="row" key={`${index}:${line.content}`}>
          <span className="review-line-no" role="cell">{line.oldLine ?? ""}</span>
          <span className="review-line-no" role="cell">{line.newLine ?? ""}</span>
          <code className="review-line-code" role="cell">
            <DiffContent line={line} range={ranges.get(index)} />
          </code>
        </div>
      ))}
    </div>
  );
}

interface SplitRow {
  left?: FilePreviewLine;
  right?: FilePreviewLine;
  separator?: FilePreviewLine;
}

function splitRows(lines: FilePreviewLine[]): SplitRow[] {
  const rows: SplitRow[] = [];
  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index];
    const next = lines[index + 1];
    if (line.kind === "separator") {
      rows.push({ separator: line });
    } else if (line.kind === "del" && next?.kind === "add") {
      rows.push({ left: line, right: next });
      index += 1;
    } else if (line.kind === "del") {
      rows.push({ left: line });
    } else if (line.kind === "add" || line.kind === "created") {
      rows.push({ right: line });
    } else {
      rows.push({ left: line, right: line });
    }
  }
  return rows;
}

function SplitSide({ line, side, wrap }: {
  line?: FilePreviewLine;
  side: "old" | "new";
  wrap: boolean;
}) {
  return (
    <div className={`review-split-side${line ? ` ft-diff-${line.kind}` : " empty"}`}>
      <span className="review-line-no">
        {side === "old" ? line?.oldLine ?? "" : line?.newLine ?? ""}
      </span>
      <code className={`review-line-code${wrap ? " wrap" : ""}`}>{line?.content ?? " "}</code>
    </div>
  );
}

function SplitPane({ rows, side, wrap }: {
  rows: SplitRow[];
  side: "old" | "new";
  wrap: boolean;
}) {
  return (
    <div className="review-split-pane" role="rowgroup">
      {rows.map((row, index) => row.separator ? (
        <div className="review-split-separator" key={`${side}:${index}:${row.separator.content}`}>
          {row.separator.content}
        </div>
      ) : (
        <div className="review-split-row" role="row" key={`${side}:${index}`}>
          <SplitSide
            line={side === "old" ? row.left : row.right}
            side={side}
            wrap={wrap}
          />
        </div>
      ))}
    </div>
  );
}

function SplitDiff({ lines, wrap }: { lines: FilePreviewLine[]; wrap: boolean }) {
  const rows = splitRows(lines);
  return (
    <div className="review-split" role="table" aria-label="左右差异">
      <SplitPane rows={rows} side="old" wrap={wrap} />
      <SplitPane rows={rows} side="new" wrap={wrap} />
    </div>
  );
}

interface FileTreeRow {
  key: string;
  kind: "directory" | "file";
  label: string;
  path: string;
  level: number;
}

function fileTreeRows(files: ReviewFileChange[]): FileTreeRow[] {
  const rows: FileTreeRow[] = [];
  const seenDirectories = new Set<string>();
  for (const file of [...files].sort((left, right) => left.path.localeCompare(right.path))) {
    const parts = file.path.split(/[\\/]/).filter(Boolean);
    for (let index = 0; index < parts.length - 1; index += 1) {
      const directory = parts.slice(0, index + 1).join("/");
      if (seenDirectories.has(directory)) continue;
      seenDirectories.add(directory);
      rows.push({
        key: `directory:${directory}`,
        kind: "directory",
        label: parts[index],
        path: directory,
        level: index + 1,
      });
    }
    rows.push({
      key: `file:${file.path}`,
      kind: "file",
      label: parts[parts.length - 1] ?? file.path,
      path: file.path,
      level: parts.length,
    });
  }
  return rows;
}

function FileTypeIcon({ path }: { path: string }) {
  return /\.(?:md|mdx|txt|rst)$/i.test(path)
    ? <FileText className="review-file-type" size={13} aria-hidden />
    : <FileCode2 className="review-file-type" size={13} aria-hidden />;
}

function richPreviewText(file: ReviewFileChange): string {
  return file.preview.flatMap((line) => {
    if (line.kind === "separator" || line.kind === "del") return [];
    if ((line.kind === "add" || line.kind === "context") && line.content.length > 0) {
      return [line.content.slice(1)];
    }
    return [line.content];
  }).join("\n");
}

function supportsRichPreview(path: string): boolean {
  return /\.(?:md|mdx)$/i.test(path);
}

function ToolbarButton({
  label,
  children,
  onClick,
  pressed,
  expanded,
  popup,
}: {
  label: string;
  children: ReactNode;
  onClick: () => void;
  pressed?: boolean;
  expanded?: boolean;
  popup?: "dialog" | "menu";
}) {
  return (
    <button
      type="button"
      className="icon-button review-tool-button"
      aria-label={label}
      title={label}
      aria-pressed={pressed}
      aria-expanded={popup ? expanded : undefined}
      aria-haspopup={popup}
      onClick={onClick}
    >
      {children}
    </button>
  );
}

export function ReviewPanel({ target, items, sourceState, refreshKey, onRefresh }: ReviewPanelProps) {
  const files = useMemo(
    () => target && items ? reviewFilesForMessage(items, target.messageId) : [],
    [items, refreshKey, target],
  );
  const targetKey = target
    ? `${target.sessionId}:${target.agentId ?? "main"}:${target.messageId}:${target.selectedPath ?? ""}`
    : "none";
  const fileKey = files.map((file) => file.path).join("\u0000");
  const [layout, setLayout] = useState<DiffLayout>("unified");
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [selectedPath, setSelectedPath] = useState<string | null>(null);
  const [treeOpen, setTreeOpen] = useState(true);
  const [jumpOpen, setJumpOpen] = useState(false);
  const [menuOpen, setMenuOpen] = useState(false);
  const [jumpQuery, setJumpQuery] = useState("");
  const [treeQuery, setTreeQuery] = useState("");
  const [wrap, setWrap] = useState(false);
  const [fullFiles, setFullFiles] = useState(false);
  const [richPreview, setRichPreview] = useState(false);
  const [wordDiff, setWordDiff] = useState(false);
  const [hideWhitespace, setHideWhitespace] = useState(false);
  const [notice, setNotice] = useState("");
  const jumpRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    const path = target?.selectedPath && files.some((file) => file.path === target.selectedPath)
      ? target.selectedPath
      : (files[0]?.path ?? null);
    setSelectedPath(path);
    setExpanded(new Set(files.map((file) => file.path)));
    setJumpOpen(false);
    setMenuOpen(false);
    setJumpQuery("");
    setTreeQuery("");
    setNotice("");
    setRichPreview(false);
  }, [fileKey, targetKey]);

  const jumpTo = (path: string) => {
    setSelectedPath(path);
    setExpanded((current) => new Set(current).add(path));
    setJumpOpen(false);
    setJumpQuery("");
    window.requestAnimationFrame(() => {
      document.getElementById(reviewFileId(path))?.scrollIntoView?.({ block: "start", behavior: "smooth" });
    });
  };

  const filteredJumpFiles = files.filter((file) =>
    file.path.toLowerCase().includes(jumpQuery.trim().toLowerCase()),
  );
  const filteredTreeFiles = files.filter((file) =>
    file.path.toLowerCase().includes(treeQuery.trim().toLowerCase()),
  );

  if (target === null) {
    return <p className="information-empty">从最终修改文件列表选择一个文件开始 Review。</p>;
  }
  if (sourceState === "loading") {
    return <p className="information-empty" role="status">正在加载可用差异…</p>;
  }
  if (terminalState(sourceState)) {
    return (
      <p className="information-empty" role="note">
        {sourceState === "missing"
          ? "该子代理没有可读取的落盘历史，无法生成逐条 Review；这里只保留代理结果快照。"
          : "子代理历史加载失败，当前无法生成 Review。"}
      </p>
    );
  }
  if (files.length === 0) {
    return <p className="information-empty">这条最终回答没有可用的已提交文件差异。</p>;
  }

  return (
    <div className="review-panel">
      <div className="review-toolbar">
        <div className="review-layout-switch" role="group" aria-label="差异布局">
          <button type="button" aria-pressed={layout === "unified"} onClick={() => setLayout("unified")}>
            <AlignJustify size={14} aria-hidden />统一差异
          </button>
          <button type="button" aria-pressed={layout === "split"} onClick={() => setLayout("split")}>
            <Columns2 size={14} aria-hidden />左右差异
          </button>
        </div>
        <span className="review-toolbar-spacer" />
        <div className="review-popover-wrap" ref={jumpRef}>
          <ToolbarButton
            label="跳转文件"
            expanded={jumpOpen}
            popup="dialog"
            onClick={() => setJumpOpen((open) => !open)}
          >
            <FileSearch size={15} aria-hidden />
          </ToolbarButton>
          {jumpOpen ? (
            <div className="review-jump-menu" role="dialog" aria-label="跳转文件">
              <label className="review-search">
                <Search size={13} aria-hidden />
                <input
                  autoFocus
                  aria-label="搜索文件"
                  placeholder="搜索文件"
                  value={jumpQuery}
                  onChange={(event) => setJumpQuery(event.target.value)}
                />
              </label>
              <div className="review-jump-results">
                {filteredJumpFiles.map((file) => (
                  <button type="button" key={file.path} onClick={() => jumpTo(file.path)}>
                    <span>{file.path.split(/[\\/]/).filter(Boolean).slice(-1)[0] ?? file.path}</span>
                    <small>{file.path.replace(/[^\\/]+$/, "")}</small>
                  </button>
                ))}
              </div>
            </div>
          ) : null}
        </div>
        <ToolbarButton
          label={expanded.size === files.length ? "全部收起" : "全部展开"}
          onClick={() => setExpanded(
            expanded.size === files.length ? new Set() : new Set(files.map((file) => file.path)),
          )}
        >
          {expanded.size === files.length
            ? <ChevronsDownUp size={15} aria-hidden />
            : <ChevronsUpDown size={15} aria-hidden />}
        </ToolbarButton>
        <ToolbarButton
          label={treeOpen ? "关闭文件树" : "打开文件树"}
          pressed={treeOpen}
          onClick={() => setTreeOpen((open) => !open)}
        >
          <FolderOpen size={15} aria-hidden />
        </ToolbarButton>
        <div className="review-popover-wrap">
          <ToolbarButton
            label="更多 Review 选项"
            expanded={menuOpen}
            popup="menu"
            onClick={() => setMenuOpen((open) => !open)}
          >
            <MoreHorizontal size={16} aria-hidden />
          </ToolbarButton>
          {menuOpen ? (
            <div className="review-options-menu" role="menu" aria-label="Review 选项">
              <button type="button" role="menuitem" onClick={() => {
                onRefresh();
                setNotice("已刷新当前转录中的修改。");
                setMenuOpen(false);
              }}><RefreshCw size={13} aria-hidden />刷新</button>
              <button type="button" role="menuitem" onClick={() => {
                setWrap((value) => !value);
                setMenuOpen(false);
              }}><WrapText size={13} aria-hidden />{wrap ? "关闭换行" : "启用换行"}</button>
              <button type="button" role="menuitem" onClick={() => {
                setFullFiles((value) => !value);
                setMenuOpen(false);
              }}><FileCode2 size={13} aria-hidden />{fullFiles ? "精简差异" : "加载完整文件"}</button>
              {files.some((file) => supportsRichPreview(file.path)) ? (
                <button type="button" role="menuitem" onClick={() => {
                  setRichPreview((value) => !value);
                  setMenuOpen(false);
                }}><FileText size={13} aria-hidden />{richPreview ? "关闭富文本预览" : "启用富文本预览"}</button>
              ) : null}
              <button type="button" role="menuitem" onClick={() => {
                setWordDiff((value) => !value);
                setMenuOpen(false);
              }}><ChevronDown size={13} aria-hidden />{wordDiff ? "关闭字级差异" : "启用字级差异"}</button>
              <button type="button" role="menuitem" onClick={() => {
                setHideWhitespace((value) => !value);
                setMenuOpen(false);
              }}><ChevronRight size={13} aria-hidden />{hideWhitespace ? "显示空白改动" : "隐藏空白改动"}</button>
              <button type="button" role="menuitem" onClick={() => {
                void navigator.clipboard?.writeText("git apply neo-review.patch");
                setNotice("已复制应用命令：git apply neo-review.patch");
                setMenuOpen(false);
              }}><Copy size={13} aria-hidden />复制应用命令</button>
            </div>
          ) : null}
        </div>
      </div>
      {notice ? <p className="review-notice" role="status">{notice}</p> : null}
      <div className={`review-workspace${treeOpen ? " tree-open" : ""}`}>
        <div className="review-diff-scroll">
          {files.map((file) => {
            const open = expanded.has(file.path);
            const hiddenWhitespace = hideWhitespace ? whitespaceOnlyPairs(file.preview) : new Set<number>();
            const visibleLines = compactLines(
              file.preview.filter((_line, index) => !hiddenWhitespace.has(index)),
              fullFiles,
            );
            return (
              <section
                id={reviewFileId(file.path)}
                className={`review-file${selectedPath === file.path ? " selected" : ""}`}
                key={file.path}
              >
                <button
                  type="button"
                  className="review-file-header"
                  aria-expanded={open}
                  onClick={() => {
                    setSelectedPath(file.path);
                    setExpanded((current) => {
                      const next = new Set(current);
                      if (next.has(file.path)) next.delete(file.path);
                      else next.add(file.path);
                      return next;
                    });
                  }}
                >
                  <ChevronRight className={open ? "open" : ""} size={14} aria-hidden />
                  <FileTypeIcon path={file.path} />
                  <span>{file.path}</span>
                  <small><b>+{file.added}</b> <i>−{file.removed}</i></small>
                </button>
                {open ? (
                  <div className="review-file-body">
                    {fullFiles && !file.created ? (
                      <p className="review-data-note">
                        当前协议没有完整文件正文，以下显示全部可用差异。
                      </p>
                    ) : null}
                    <div className="review-code-scroll">
                      {layout === "unified" ? (
                        <UnifiedDiff lines={visibleLines} wordDiff={wordDiff} wrap={wrap} />
                      ) : (
                        <SplitDiff lines={visibleLines} wrap={wrap} />
                      )}
                    </div>
                    {richPreview && supportsRichPreview(file.path) ? (
                      <section className="review-rich-preview" aria-label={`${file.path} 的富文本预览`}>
                        <Markdown text={richPreviewText(file)} />
                      </section>
                    ) : null}
                  </div>
                ) : null}
              </section>
            );
          })}
        </div>
        {treeOpen ? (
          <aside className="review-tree" aria-label="修改文件树">
            <label className="review-search">
              <Search size={13} aria-hidden />
              <input
                type="search"
                aria-label="筛选文件树"
                placeholder="筛选文件"
                value={treeQuery}
                onChange={(event) => setTreeQuery(event.target.value)}
              />
            </label>
            <div role="tree" className="review-tree-list">
              {fileTreeRows(filteredTreeFiles).map((row) => row.kind === "directory" ? (
                <div
                  key={row.key}
                  role="treeitem"
                  aria-level={row.level}
                  aria-expanded="true"
                  className="review-tree-directory"
                  style={{ paddingLeft: `${(row.level - 1) * 12}px` }}
                >
                  <FolderOpen size={13} aria-hidden />{row.label}
                </div>
              ) : (
                <button
                  key={row.key}
                  type="button"
                  role="treeitem"
                  aria-level={row.level}
                  aria-selected={selectedPath === row.path}
                  className="review-tree-file"
                  style={{ paddingLeft: `${(row.level - 1) * 12}px` }}
                  title={row.path}
                  onClick={() => jumpTo(row.path)}
                >
                  <FileTypeIcon path={row.path} />{row.label}
                </button>
              ))}
            </div>
          </aside>
        ) : null}
      </div>
    </div>
  );
}

/**
 * In-app folder picker. The service runs on the same machine as the browser,
 * so the browser cannot open a native OS directory dialog; instead this
 * component browses directories through the backend `/api/fs/list` endpoint,
 * which reads the local filesystem server-side. Directories only — files are
 * shown dimmed for orientation and are never selectable.
 */

import { useCallback, useEffect, useState } from "react";
import { File, Folder, FolderUp, Home, X } from "lucide-react";
import { listDirectory } from "../api";
import type { WebUiFsListing } from "../protocol";

interface Breadcrumb {
  label: string;
  path: string;
}

function buildBreadcrumbs(path: string): Breadcrumb[] {
  if (path.startsWith("/")) {
    const parts = path.split("/").filter((part) => part.length > 0);
    const crumbs: Breadcrumb[] = [{ label: "/", path: "/" }];
    let acc = "";
    for (const part of parts) {
      acc += `/${part}`;
      crumbs.push({ label: part, path: acc });
    }
    return crumbs;
  }
  if (/^[A-Za-z]:[\\/]/.test(path)) {
    const drive = path.slice(0, 2);
    const rest = path
      .slice(2)
      .split(/[\\/]/)
      .filter((part) => part.length > 0);
    const crumbs: Breadcrumb[] = [{ label: drive, path: `${drive}\\` }];
    let acc = drive;
    for (const part of rest) {
      acc += `\\${part}`;
      crumbs.push({ label: part, path: acc });
    }
    return crumbs;
  }
  return [{ label: path, path }];
}

export function FolderBrowser({
  onClose,
  onSelect,
}: {
  onClose: () => void;
  onSelect: (path: string) => void;
}) {
  const [listing, setListing] = useState<WebUiFsListing | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback((path?: string) => {
    setLoading(true);
    setError(null);
    listDirectory(path)
      .then((next) => setListing(next))
      .catch(() => setError("无法读取该目录。请检查路径或权限。"))
      .finally(() => setLoading(false));
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  const directories = listing?.entries.filter((entry) => entry.is_dir) ?? [];
  const files = listing?.entries.filter((entry) => !entry.is_dir) ?? [];
  const breadcrumbs = listing === null ? [] : buildBreadcrumbs(listing.path);

  return (
    <div className="dialog-backdrop" role="presentation">
      <div className="folder-browser" role="dialog" aria-modal="true" aria-label="选择文件夹">
        <div className="folder-browser-header">
          <h2>选择文件夹</h2>
          <button type="button" className="icon-button" aria-label="关闭" onClick={onClose}>
            <X size={16} aria-hidden />
          </button>
        </div>

        <div className="folder-browser-breadcrumbs" aria-label="当前路径">
          {breadcrumbs.map((crumb, index) => (
            <span key={`${crumb.path}:${index}`} className="folder-browser-crumb">
              {index > 0 ? <span className="folder-browser-sep">/</span> : null}
              <button
                type="button"
                onClick={() => load(crumb.path)}
                title={crumb.path}
              >
                {crumb.label}
              </button>
            </span>
          ))}
        </div>

        <div className="folder-browser-toolbar">
          <button
            type="button"
            className="chip-button"
            disabled={loading || listing?.parent === undefined || listing?.parent === null}
            onClick={() => listing?.parent !== undefined && listing.parent !== null && load(listing.parent)}
          >
            <FolderUp size={14} aria-hidden />
            上一级
          </button>
          <button
            type="button"
            className="chip-button"
            disabled={loading}
            onClick={() => load()}
            title="回到主目录"
          >
            <Home size={14} aria-hidden />
            主目录
          </button>
        </div>

        <div className="folder-browser-list" role="listbox" aria-label="文件夹列表">
          {loading ? (
            <div className="folder-browser-empty">加载中…</div>
          ) : error !== null ? (
            <div className="folder-browser-empty" role="alert">{error}</div>
          ) : directories.length === 0 && files.length === 0 ? (
            <div className="folder-browser-empty">此文件夹为空</div>
          ) : null}
          {directories.map((entry) => (
            <button
              key={entry.name}
              type="button"
              role="option"
              aria-selected="false"
              className="folder-browser-row"
              onClick={() => listing !== null && load(drill(listing.path, entry.name))}
              onDoubleClick={() => listing !== null && load(drill(listing.path, entry.name))}
            >
              <Folder size={15} aria-hidden className="folder-browser-folder" />
              <span className="folder-browser-name">{entry.name}</span>
            </button>
          ))}
          {files.map((entry) => (
            <div key={entry.name} className="folder-browser-row is-file" aria-hidden>
              <File size={15} className="folder-browser-folder" />
              <span className="folder-browser-name">{entry.name}</span>
            </div>
          ))}
        </div>

        <div className="folder-browser-actions">
          <button type="button" onClick={onClose}>
            取消
          </button>
          <button
            type="button"
            className="primary-button"
            disabled={loading || listing === null}
            onClick={() => listing !== null && onSelect(listing.path)}
          >
            选择此文件夹
          </button>
        </div>
      </div>
    </div>
  );
}

function drill(path: string, name: string): string {
  const sep = path.includes("\\") ? "\\" : "/";
  if (path.endsWith(sep)) return `${path}${name}`;
  return `${path}${sep}${name}`;
}

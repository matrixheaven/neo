/**
 * Add-project dialog: a local path typed by the user (the service runs on
 * the same machine, so the browser cannot open a native folder picker).
 * Shared by the sidebar "添加项目" entry and the composer workspace
 * picker's "打开新文件夹作为工作区" footer action.
 */

import { useState } from "react";
import type { WebUiWorkspaceGroup } from "../protocol";
import { useAppActions } from "../state/store";
import { FolderBrowser } from "./folderBrowser";

export function AddWorkspaceDialog({
  onClose,
  onAdded,
}: {
  onClose: () => void;
  onAdded?: (workspace: WebUiWorkspaceGroup) => void;
}) {
  const actions = useAppActions();
  const [path, setPath] = useState("");
  const [saving, setSaving] = useState(false);
  const [browsing, setBrowsing] = useState(false);

  return (
    <div className="dialog-backdrop" role="presentation">
      {browsing ? (
        <FolderBrowser
          onClose={() => setBrowsing(false)}
          onSelect={(selected) => {
            setPath(selected);
            setBrowsing(false);
          }}
        />
      ) : null}
      <form
        className="workspace-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="workspace-dialog-title"
        onSubmit={(event) => {
          event.preventDefault();
          const trimmed = path.trim();
          if (trimmed === "" || saving) return;
          setSaving(true);
          actions
            .addWorkspace(trimmed)
            .then((workspace) => {
              onAdded?.(workspace);
              onClose();
            })
            .catch(() => {})
            .finally(() => setSaving(false));
        }}
      >
        <h2 id="workspace-dialog-title">添加项目</h2>
        <label htmlFor="workspace-path">项目文件夹</label>
        <div className="workspace-path-row">
          <input
            id="workspace-path"
            autoFocus
            value={path}
            placeholder="/Users/name/Workspace/project"
            onChange={(event) => setPath(event.target.value)}
          />
          <button type="button" onClick={() => setBrowsing(true)}>
            浏览…
          </button>
        </div>
        <div className="workspace-dialog-actions">
          <button type="button" onClick={onClose}>
            取消
          </button>
          <button type="submit" className="primary-button" disabled={saving}>
            {saving ? "添加中…" : "添加"}
          </button>
        </div>
      </form>
    </div>
  );
}

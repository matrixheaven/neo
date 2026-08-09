/**
 * Full tool/terminal output loaded on demand through the opaque output
 * reference. The id is passed back verbatim — never encoded, decoded or
 * guessed.
 */

import { useEffect, useState } from "react";
import { ApiError, readToolOutput } from "../api";
import type { WebUiOutputRef } from "../protocol";
import { OutputBlock } from "./codeBlock";

export function FullOutput({
  sessionId,
  itemId,
  outputRef,
}: {
  sessionId: string;
  itemId: string;
  outputRef: WebUiOutputRef;
}) {
  const [state, setState] = useState<
    | { kind: "idle" }
    | { kind: "loading" }
    | { kind: "loaded"; text: string; truncatedNote: string | null }
    | { kind: "error"; message: string }
  >({ kind: "idle" });

  useEffect(() => {
    setState({ kind: "idle" });
  }, [outputRef.id]);

  const load = () => {
    setState({ kind: "loading" });
    readToolOutput(sessionId, outputRef.id)
      .then((range) => {
        setState({
          kind: "loaded",
          text: range.text,
          truncatedNote: range.reached_end ? null : `仅显示到第 ${range.next_line} 行`,
        });
      })
      .catch((error: unknown) => {
        const message =
          error instanceof ApiError && error.code === "output_not_in_session"
            ? "输出不属于当前会话"
            : "无法读取完整输出";
        setState({ kind: "error", message });
      });
  };

  return (
    <div className="full-output" data-item={itemId}>
      <p className="full-output-meta">
        完整输出：{outputRef.line_count} 行 / {outputRef.byte_len} 字节
        {outputRef.complete ? "" : "（服务端仍在追加）"}
      </p>
      {state.kind === "idle" ? (
        <button type="button" className="text-action" onClick={load}>
          读取完整输出
        </button>
      ) : null}
      {state.kind === "loading" ? <p className="muted">读取中…</p> : null}
      {state.kind === "error" ? <p className="error-text">{state.message}</p> : null}
      {state.kind === "loaded" ? (
        <>
          {state.truncatedNote ? <p className="muted">{state.truncatedNote}</p> : null}
          <OutputBlock text={state.text} />
        </>
      ) : null}
    </div>
  );
}

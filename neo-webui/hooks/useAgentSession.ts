'use client';
import { useState, useCallback, useRef, useEffect } from 'react';
import type { AgentEvent, ApprovalRequest } from '@/lib/types';

export interface ToolCallInfo {
  id: string;
  name: string;
  arguments: string;
  result?: { content: string; isError: boolean; duration_ms?: number };
  status: 'pending' | 'running' | 'done';
  startedAt?: number;
  duration_ms?: number;
}

export interface Message {
  id: string;
  role: 'user' | 'assistant';
  content: string;
  thinking?: { text: string; collapsed: boolean };
  toolCalls?: ToolCallInfo[];
}

interface UseAgentSessionReturn {
  messages: Message[];
  isStreaming: boolean;
  error: string | null;
  sendMessage: (text: string) => Promise<void>;
  clearMessages: () => void;
  pendingApproval: ApprovalRequest | null;
  approveTool: (action: string) => Promise<void>;
  rejectTool: () => Promise<void>;
}

const generateId = (): string => {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') {
    return crypto.randomUUID();
  }
  return `${Math.random().toString(36).slice(2)}${Math.random().toString(36).slice(2)}`;
};

export function useAgentSession(sessionId: string): UseAgentSessionReturn {
  const [messages, setMessages] = useState<Message[]>([]);
  const [isStreaming, setIsStreaming] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [pendingApproval, setPendingApproval] = useState<ApprovalRequest | null>(null);
  const eventSourceRef = useRef<EventSource | null>(null);
  const messagesRef = useRef<Message[]>([]);

  const connectSSE = useCallback(() => {
    if (eventSourceRef.current) {
      eventSourceRef.current.close();
    }

    const es = new EventSource(`/api/sessions/${sessionId}/events`);
    eventSourceRef.current = es;

    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    es.addEventListener('agent.event', (e: any) => {
      const event: AgentEvent = JSON.parse(e.data as string);
      const msgs = messagesRef.current;

      if ('TextDelta' in event) {
        const last = msgs[msgs.length - 1];
        if (last && last.role === 'assistant') {
          last.content += event.TextDelta.text;
        }
      } else if ('ThinkingDelta' in event) {
        const last = msgs[msgs.length - 1];
        if (last && last.role === 'assistant' && last.thinking) {
          last.thinking.text += event.ThinkingDelta.text;
        }
      } else if ('ThinkingStarted' in event) {
        const last = msgs[msgs.length - 1];
        if (last && last.role === 'assistant') {
          last.thinking = { text: '', collapsed: false };
        }
      } else if ('ThinkingFinished' in event) {
        const last = msgs[msgs.length - 1];
        if (last && last.thinking) {
          last.thinking.collapsed = true;
        }
      } else if ('MessageStarted' in event) {
        msgs.push({ id: event.MessageStarted.id, role: 'assistant', content: '' });
      } else if ('MessageFinished' in event) {
        setIsStreaming(false);
      } else if ('ToolCallStarted' in event) {
        const last = msgs[msgs.length - 1];
        if (last && last.role === 'assistant') {
          if (!last.toolCalls) last.toolCalls = [];
          last.toolCalls.push({
            id: event.ToolCallStarted.id,
            name: event.ToolCallStarted.name,
            arguments: '',
            status: 'pending',
          });
        }
      } else if ('ToolCallArgumentsDelta' in event) {
        const last = msgs[msgs.length - 1];
        const tc = last?.toolCalls?.find(t => t.id === event.ToolCallArgumentsDelta.id);
        if (tc) {
          tc.arguments += event.ToolCallArgumentsDelta.json_fragment;
          tc.status = 'running';
        }
      } else if ('ToolCallFinished' in event) {
        const last = msgs[msgs.length - 1];
        const tc = last?.toolCalls?.find(t => t.id === event.ToolCallFinished.tool_call.id);
        if (tc) {
          tc.arguments = event.ToolCallFinished.tool_call.arguments;
        }
      } else if ('ToolExecutionStarted' in event) {
        const last = msgs[msgs.length - 1];
        const tc = last?.toolCalls?.find(t => t.id === event.ToolExecutionStarted.id);
        if (tc) {
          tc.status = 'running';
          tc.startedAt = Date.now();
        }
      } else if ('ToolExecutionFinished' in event) {
        const last = msgs[msgs.length - 1];
        const tc = last?.toolCalls?.find(t => t.id === event.ToolExecutionFinished.id);
        if (tc) {
          tc.status = 'done';
          tc.duration_ms = tc.startedAt ? Date.now() - tc.startedAt : undefined;
          tc.result = {
            content: event.ToolExecutionFinished.result.content,
            isError: event.ToolExecutionFinished.result.isError,
          };
        }
      } else if ('ApprovalRequested' in event) {
        setPendingApproval(event.ApprovalRequested.request);
        setMessages([...msgs]);
      } else if ('ApprovalResolved' in event) {
        setPendingApproval(null);
      } else if ('Error' in event) {
        setError(event.Error.message);
        setIsStreaming(false);
      }

      setMessages([...msgs]);
    });

    es.addEventListener('error', () => {
      // SSE will auto-reconnect
    });

    es.addEventListener('closed', () => {
      setIsStreaming(false);
    });
  }, [sessionId]);

  useEffect(() => {
    connectSSE();
    return () => {
      eventSourceRef.current?.close();
    };
  }, [connectSSE]);

  const sendMessage = useCallback(async (text: string) => {
    setError(null);
    const userMsg: Message = { id: generateId(), role: 'user', content: text };
    messagesRef.current = [...messagesRef.current, userMsg];
    setMessages([...messagesRef.current]);
    setIsStreaming(true);

    await fetch(`/api/sessions/${sessionId}/prompt`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ message: text }),
    });
    // SSE will deliver the response
  }, [sessionId]);

  const clearMessages = useCallback(() => {
    messagesRef.current = [];
    setMessages([]);
  }, []);

  const approveTool = useCallback(async (action: string) => {
    if (!pendingApproval) return;
    await fetch(`/api/sessions/${sessionId}/approve`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ request_id: pendingApproval.id, action }),
    });
    setPendingApproval(null);
  }, [sessionId, pendingApproval]);

  const rejectTool = useCallback(async () => {
    if (!pendingApproval) return;
    await fetch(`/api/sessions/${sessionId}/approve`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ request_id: pendingApproval.id, action: 'reject' }),
    });
    setPendingApproval(null);
  }, [sessionId, pendingApproval]);

  return { messages, isStreaming, error, sendMessage, clearMessages, pendingApproval, approveTool, rejectTool };
}

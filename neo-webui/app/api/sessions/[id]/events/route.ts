import { NextRequest } from 'next/server';
import { sessionRegistry } from '@/lib/session-registry';

export async function GET(
  request: NextRequest,
  { params }: { params: Promise<{ id: string }> }
) {
  const { id } = await params;

  // Create a TransformStream for SSE
  const encoder = new TextEncoder();

  const stream = new ReadableStream({
    async start(controller) {
      // Helper to send SSE formatted event
      const sendEvent = (event: string, data: unknown) => {
        const line = `event: ${event}\ndata: ${JSON.stringify(data)}\n\n`;
        controller.enqueue(encoder.encode(line));
      };

      try {
        const entry = await sessionRegistry.ensure(id);

        // Send initial connection event
        sendEvent('connected', { session_id: id });

        // Forward RPC notifications as SSE events
        const onNotification = (method: string, params: unknown) => {
          if (method === 'agent.event') {
            sendEvent('agent.event', params);
          } else {
            sendEvent(method, params);
          }
        };

        const onClose = (code: number | null) => {
          sendEvent('closed', { code, session_id: id });
          controller.close();
        };

        entry.process.on('notification', onNotification);
        entry.process.on('close', onClose);

        // Keep stream alive until client disconnects
        const keepAlive = setInterval(() => {
          try {
            controller.enqueue(encoder.encode(': keepalive\n\n'));
          } catch {
            clearInterval(keepAlive);
          }
        }, 15000);

        // Cleanup on abort
        request.signal.addEventListener('abort', () => {
          clearInterval(keepAlive);
          entry.process.off('notification', onNotification);
          entry.process.off('close', onClose);
          controller.close();
        });
      } catch (err: any) {
        sendEvent('error', { message: err.message ?? 'Failed to start session' });
        controller.close();
      }
    },
  });

  return new Response(stream, {
    headers: {
      'Content-Type': 'text/event-stream',
      'Cache-Control': 'no-cache',
      'Connection': 'keep-alive',
      'X-Accel-Buffering': 'no',
    },
  });
}

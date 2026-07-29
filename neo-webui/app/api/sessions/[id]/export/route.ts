import { NextRequest, NextResponse } from 'next/server';
import { NeoRpcProcess } from '@/lib/rpc-client';

export async function GET(
  request: NextRequest,
  { params }: { params: Promise<{ id: string }> }
) {
  const { id } = await params;
  const format = request.nextUrl.searchParams.get('format') ?? 'html';
  const proc = new NeoRpcProcess();
  try {
    const result = await proc.call('sessions.export', { session_id: id, format }) as { content: string };
    if (format === 'json') {
      return NextResponse.json(JSON.parse(result.content));
    }
    return new NextResponse(result.content, {
      headers: { 'Content-Type': 'text/html; charset=utf-8' },
    });
  } finally {
    proc.destroy();
  }
}

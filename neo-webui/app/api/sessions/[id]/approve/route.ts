import { NextRequest, NextResponse } from 'next/server';
import { sessionRegistry } from '@/lib/session-registry';

export async function POST(
  request: NextRequest,
  { params }: { params: Promise<{ id: string }> }
) {
  const { id } = await params;
  const { request_id, action } = await request.json();
  
  if (!request_id || !action) {
    return NextResponse.json({ error: 'request_id and action required' }, { status: 400 });
  }
  
  const entry = sessionRegistry.get(id);
  if (!entry) {
    return NextResponse.json({ error: 'session not active' }, { status: 404 });
  }
  
  await entry.process.call('approve_tool', { session_id: id, request_id, action });
  return NextResponse.json({ status: 'ok' });
}

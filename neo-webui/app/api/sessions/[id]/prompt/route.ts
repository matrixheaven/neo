import { NextRequest, NextResponse } from 'next/server';
import { sessionRegistry } from '@/lib/session-registry';

export async function POST(
  request: NextRequest,
  { params }: { params: Promise<{ id: string }> }
) {
  const { id } = await params;
  const { message } = await request.json();
  
  if (!message || typeof message !== 'string') {
    return NextResponse.json({ error: 'message is required' }, { status: 400 });
  }
  
  const entry = await sessionRegistry.ensure(id);
  entry.process.call('prompt', { session_id: id, message }).catch(err => {
    console.error('prompt error:', err);
  });
  
  return NextResponse.json({ status: 'sent' });
}

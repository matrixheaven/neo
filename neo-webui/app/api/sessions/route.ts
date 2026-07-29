import { NextRequest, NextResponse } from 'next/server';
import * as crypto from 'crypto';
import { listAllSessions } from '@/lib/session-index';
import { sessionRegistry } from '@/lib/session-registry';

export async function GET() {
  const groups = listAllSessions();
  return NextResponse.json({ groups });
}

export async function POST(request: NextRequest) {
  const { workdir } = await request.json().catch(() => ({}));
  if (!workdir || typeof workdir !== 'string') {
    return NextResponse.json({ error: 'workdir is required' }, { status: 400 });
  }
  const sessionId = `session_${crypto.randomUUID()}`;
  await sessionRegistry.ensure(sessionId, workdir);
  return NextResponse.json({ session_id: sessionId, workdir });
}

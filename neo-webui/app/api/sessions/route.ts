import { NextResponse } from 'next/server';
import { listAllSessions } from '@/lib/session-index';

export async function GET() {
  const groups = listAllSessions();
  return NextResponse.json({ groups });
}

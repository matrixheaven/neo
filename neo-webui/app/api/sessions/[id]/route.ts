import { NextRequest, NextResponse } from 'next/server';
import { NeoRpcProcess } from '@/lib/rpc-client';

export async function GET(
  request: NextRequest,
  { params }: { params: Promise<{ id: string }> }
) {
  const { id } = await params;
  const proc = new NeoRpcProcess();
  try {
    const result = await proc.call('sessions.get', { session_id: id });
    return NextResponse.json(result);
  } finally {
    proc.destroy();
  }
}

export async function PATCH(
  request: NextRequest,
  { params }: { params: Promise<{ id: string }> }
) {
  const { id } = await params;
  const body = await request.json();
  const proc = new NeoRpcProcess();
  try {
    const result = await proc.call('set_session_name', { session_id: id, name: body.name });
    return NextResponse.json(result);
  } finally {
    proc.destroy();
  }
}

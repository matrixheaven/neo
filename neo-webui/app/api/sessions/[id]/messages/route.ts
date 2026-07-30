import { NextRequest, NextResponse } from 'next/server';
import { NeoRpcProcess } from '@/lib/rpc-client';
import { findSessionWorkdir } from '@/lib/session-index';

export async function GET(
  request: NextRequest,
  { params }: { params: Promise<{ id: string }> }
) {
  const { id } = await params;
  const workdir = findSessionWorkdir(id) ?? undefined;
  const proc = new NeoRpcProcess('neo', workdir);
  try {
    const result = await Promise.race([
      proc.call('get_messages', { session_id: id }),
      new Promise<never>((_, reject) => setTimeout(() => reject(new Error('Timeout after 10s')), 10_000)),
    ]) as {
      session_id: string;
      messages: unknown[];
    };
    console.error(`[messages] session=${id} count=${result.messages?.length ?? 0}`);
    return NextResponse.json(result);
  } catch (err: any) {
    console.error(`[messages] session=${id} error=`, err.message ?? err);
    return NextResponse.json({ session_id: id, messages: [] });
  } finally {
    proc.destroy();
  }
}

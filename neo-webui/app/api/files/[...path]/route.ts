import { NextRequest, NextResponse } from 'next/server';
import { resolveFile, readTextFile } from '@/lib/file-access';

export async function GET(
  request: NextRequest,
  { params }: { params: Promise<{ path: string[] }> }
) {
  const { path: pathSegments } = await params;
  const subpath = pathSegments.join('/');
  const { searchParams } = new URL(request.url);
  const root = searchParams.get('root') || undefined;
  const resolved = resolveFile(subpath, root);
  if (!resolved.valid) {
    return NextResponse.json({ error: resolved.error }, { status: 403 });
  }
  const result = readTextFile(resolved.fullPath!, subpath || undefined);
  if (result.error) {
    return NextResponse.json({ error: result.error }, { status: 404 });
  }
  return NextResponse.json({ content: result.content });
}

import { NextRequest, NextResponse } from 'next/server';
import { resolveFile, readTextFile } from '@/lib/file-access';

export async function GET(
  request: NextRequest,
  { params }: { params: Promise<{ path: string[] }> }
) {
  const { path: pathSegments } = await params;
  const subpath = pathSegments.join('/');
  const resolved = resolveFile(subpath);
  if (!resolved.valid) {
    return NextResponse.json({ error: resolved.error }, { status: 403 });
  }
  const result = readTextFile(resolved.fullPath!);
  if (result.error) {
    return NextResponse.json({ error: result.error }, { status: 404 });
  }
  return NextResponse.json({ content: result.content });
}

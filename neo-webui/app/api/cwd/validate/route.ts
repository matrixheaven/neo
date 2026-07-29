import { NextRequest, NextResponse } from 'next/server';
import * as fs from 'fs';

export async function POST(request: NextRequest) {
  const { path: dirPath } = await request.json();
  if (!dirPath || typeof dirPath !== 'string') {
    return NextResponse.json({ error: 'path required' }, { status: 400 });
  }
  const exists = fs.existsSync(dirPath) && fs.statSync(dirPath).isDirectory();
  return NextResponse.json({ exists });
}

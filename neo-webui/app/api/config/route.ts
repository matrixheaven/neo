import { NextResponse } from 'next/server';
import { readConfig } from '@/lib/config-reader';

export async function GET() {
  const { config, skills } = readConfig();
  return NextResponse.json({ config, skills });
}

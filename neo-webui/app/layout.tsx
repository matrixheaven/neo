import type { Metadata } from 'next';
import '../globals.css';

export const metadata: Metadata = {
  title: 'Neo WebUI',
  description: 'Browser interface for Neo AI coding agent',
};

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en">
      <body>
        <div id="__next" style={{ display: 'flex', height: '100vh', overflow: 'hidden' }}>
          {children}
        </div>
      </body>
    </html>
  );
}

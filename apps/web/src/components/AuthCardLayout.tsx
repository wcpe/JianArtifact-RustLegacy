import type { ReactNode } from 'react';
import { Card, CardContent, CardDescription, CardHeader } from '@jianartifact/ui';

interface AuthCardLayoutProps {
  title: string;
  description: string;
  children: ReactNode;
}

export function AuthCardLayout({ title, description, children }: AuthCardLayoutProps) {
  return (
    <main className="flex min-h-svh items-center justify-center bg-background p-4 sm:p-6">
      <Card className="w-full max-w-sm border-primary/15 shadow-lg shadow-primary/5">
        <CardHeader>
          <p className="text-base font-semibold text-primary">JianArtifact</p>
          <h1 className="text-xl leading-tight font-semibold">{title}</h1>
          <CardDescription className="text-base leading-relaxed">{description}</CardDescription>
        </CardHeader>
        <CardContent>{children}</CardContent>
      </Card>
    </main>
  );
}

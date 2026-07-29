'use client';
import React, { useEffect, useState } from 'react';
import type { UserSkill } from '@/lib/config-reader';

interface ModelsConfigProps {
  onClose: () => void;
}

interface ConfigData {
  raw?: string;
}

export function ModelsConfig({ onClose }: ModelsConfigProps) {
  const [config, setConfig] = useState<ConfigData | null>(null);
  const [skills, setSkills] = useState<UserSkill[]>([]);
  const [activeTab, setActiveTab] = useState<'models' | 'skills'>('models');

  useEffect(() => {
    fetch('/api/config')
      .then((r) => r.json())
      .then((data: { config?: ConfigData; skills?: UserSkill[] }) => {
        setConfig(data.config ?? null);
        setSkills(data.skills ?? []);
      })
      .catch(console.error);
  }, []);

  return (
    <div
      style={{
        position: 'fixed',
        inset: 0,
        zIndex: 200,
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
      }}
    >
      <div
        onClick={onClose}
        style={{ position: 'absolute', inset: 0, background: 'rgba(0,0,0,0.5)' }}
      />
      <div
        style={{
          position: 'relative',
          width: 640,
          maxHeight: '80vh',
          background: 'var(--color-bg-primary)',
          borderRadius: 'var(--radius-lg)',
          boxShadow: 'var(--shadow-lg)',
          display: 'flex',
          flexDirection: 'column',
        }}
      >
        <div
          style={{
            padding: 'var(--space-md) var(--space-lg)',
            borderBottom: '1px solid var(--color-border)',
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'space-between',
          }}
        >
          <h2 style={{ fontSize: 'var(--font-size-lg)', margin: 0 }}>Configuration</h2>
          <button
            onClick={onClose}
            style={{
              background: 'none',
              border: 'none',
              cursor: 'pointer',
              color: 'var(--color-text-secondary)',
              fontSize: 'var(--font-size-lg)',
            }}
          >
            ✕
          </button>
        </div>

        <div style={{ display: 'flex', borderBottom: '1px solid var(--color-border)' }}>
          <TabButton active={activeTab === 'models'} onClick={() => setActiveTab('models')}>
            Models
          </TabButton>
          <TabButton active={activeTab === 'skills'} onClick={() => setActiveTab('skills')}>
            Skills
          </TabButton>
        </div>

        <div style={{ flex: 1, overflow: 'auto', padding: 'var(--space-md)' }}>
          {activeTab === 'models' ? (
            <div>
              <p
                style={{
                  fontSize: 'var(--font-size-sm)',
                  color: 'var(--color-text-tertiary)',
                  marginBottom: 'var(--space-md)',
                }}
              >
                Raw config from ~/.neo/config.toml:
              </p>
              <pre
                style={{
                  background: 'var(--color-bg-secondary)',
                  borderRadius: 'var(--radius-md)',
                  padding: 'var(--space-md)',
                  fontSize: 'var(--font-size-sm)',
                  fontFamily: 'var(--font-mono)',
                  overflow: 'auto',
                  maxHeight: 400,
                  whiteSpace: 'pre-wrap',
                }}
              >
                {config?.raw || 'No config found'}
              </pre>
            </div>
          ) : (
            <div>
              {skills.length === 0 ? (
                <p
                  style={{
                    color: 'var(--color-text-tertiary)',
                    fontSize: 'var(--font-size-sm)',
                  }}
                >
                  No skills found
                </p>
              ) : (
                skills.map((skill: UserSkill) => (
                  <div
                    key={skill.name}
                    style={{
                      padding: 'var(--space-sm) var(--space-md)',
                      marginBottom: 'var(--space-sm)',
                      background: 'var(--color-bg-secondary)',
                      borderRadius: 'var(--radius-sm)',
                      fontSize: 'var(--font-size-sm)',
                    }}
                  >
                    <div style={{ fontWeight: 600 }}>{skill.name}</div>
                    {skill.description && (
                      <div style={{ color: 'var(--color-text-tertiary)' }}>
                        {skill.description}
                      </div>
                    )}
                    <div
                      style={{
                        color: 'var(--color-text-tertiary)',
                        fontSize: 'var(--font-size-xs, 11px)',
                        marginTop: 2,
                      }}
                    >
                      {skill.tier}
                    </div>
                  </div>
                ))
              )}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

function TabButton({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      onClick={onClick}
      style={{
        padding: 'var(--space-sm) var(--space-lg)',
        border: 'none',
        background: active ? 'var(--color-bg-primary)' : 'transparent',
        borderBottom: active ? '2px solid var(--color-accent)' : '2px solid transparent',
        color: active ? 'var(--color-accent)' : 'var(--color-text-secondary)',
        cursor: 'pointer',
        fontSize: 'var(--font-size-sm)',
        fontWeight: active ? 600 : 400,
      }}
    >
      {children}
    </button>
  );
}

import * as fs from 'fs';
import * as path from 'path';
import * as os from 'os';

function neoHome(): string {
  return process.env.NEO_HOME ?? path.join(os.homedir(), '.neo');
}

export interface UserSkill {
  name: string;
  description?: string;
  tier: string;
  path: string;
}

export function readConfig(): { config: any; skills: UserSkill[] } {
  const home = neoHome();
  let config: any = {};
  const configPath = path.join(home, 'config.toml');
  if (fs.existsSync(configPath)) {
    const raw = fs.readFileSync(configPath, 'utf-8');
    // Simple TOML-like parser for our needs (just read as-is, frontend will parse)
    config = { raw };
  }

  const skills: UserSkill[] = [];
  const skillsDir = path.join(home, 'skills');
  if (fs.existsSync(skillsDir)) {
    for (const tier of fs.readdirSync(skillsDir)) {
      const tierPath = path.join(skillsDir, tier);
      if (!fs.statSync(tierPath).isDirectory()) continue;
      for (const name of fs.readdirSync(tierPath)) {
        const skillPath = path.join(tierPath, name);
        if (!fs.statSync(skillPath).isDirectory()) continue;
        let description: string | undefined;
        const skillMdPath = path.join(skillPath, 'SKILL.md');
        if (fs.existsSync(skillMdPath)) {
          const content = fs.readFileSync(skillMdPath, 'utf-8');
          const match = content.match(/description:\s*(.+)/);
          if (match) description = match[1].trim();
        }
        skills.push({ name, description, tier, path: skillPath });
      }
    }
  }

  return { config, skills };
}

import { defineConfig } from 'vitepress'

export default defineConfig({
  title: 'Neo',
  description: 'Neo — 交互式本地编程 Agent',

  // Aegis internal design/work notes are not product docs; some contain bare
  // angle-bracket placeholders that break the Vue markdown compiler.
  srcExclude: ['aegis/**'],
  // Product docs intentionally link into the monorepo (crates/, examples/);
  // those paths are not VitePress routes.
  ignoreDeadLinks: [
    /(\.\.\/)+(crates|examples)\//,
  ],

  head: [
    ['meta', { name: 'theme-color', content: '#C678DD' }],
  ],

  locales: {
    zh: {
      label: '简体中文',
      lang: 'zh-CN',
      link: '/user_guide/zh/',
      title: 'Neo 文档',
      description: 'Neo — 交互式本地编程 Agent',
      themeConfig: {
        nav: [
          { text: '指南', link: '/user_guide/zh/guides/interaction', activeMatch: '/user_guide/zh/guides/' },
          { text: '配置', link: '/user_guide/zh/configuration/config-files', activeMatch: '/user_guide/zh/configuration/' },
          { text: '定制化', link: '/user_guide/zh/customization/mcp', activeMatch: '/user_guide/zh/customization/' },
          { text: '参考手册', link: '/user_guide/zh/reference/tools', activeMatch: '/user_guide/zh/reference/' },
        ],
        sidebar: {
          '/user_guide/zh/': [
            {
              text: '快速开始',
              items: [
                { text: '快速上手', link: '/user_guide/zh/quickstart' },
              ],
            },
            {
              text: '指南',
              items: [
                { text: '交互模式', link: '/user_guide/zh/guides/interaction' },
                { text: '会话管理', link: '/user_guide/zh/guides/sessions' },
                { text: '目标模式', link: '/user_guide/zh/guides/goals' },
                { text: '计划模式', link: '/user_guide/zh/guides/plan-mode' },
                { text: '本地工作流', link: '/user_guide/zh/guides/workflows' },
                { text: '常见用例', link: '/user_guide/zh/guides/use-cases' },
                { text: '常见问题', link: '/user_guide/zh/guides/faq' },
              ],
            },
            {
              text: '配置',
              items: [
                { text: '配置文件', link: '/user_guide/zh/configuration/config-files' },
                { text: '服务商与模型', link: '/user_guide/zh/configuration/providers' },
                { text: '权限模式', link: '/user_guide/zh/configuration/permissions' },
                { text: '数据存储位置', link: '/user_guide/zh/configuration/data-locations' },
              ],
            },
            {
              text: '定制化',
              items: [
                { text: 'MCP 服务器', link: '/user_guide/zh/customization/mcp' },
                { text: '技能（Skills）', link: '/user_guide/zh/customization/skills' },
                { text: '子 Agent', link: '/user_guide/zh/customization/agents' },
                { text: '项目指令（AGENTS.md）', link: '/user_guide/zh/customization/instructions' },
                { text: '主题（Themes）', link: '/user_guide/zh/customization/themes' },
              ],
            },
            {
              text: '参考手册',
              items: [
                { text: '内置工具', link: '/user_guide/zh/reference/tools' },
                { text: '斜杠命令', link: '/user_guide/zh/reference/slash-commands' },
                { text: '键盘快捷键', link: '/user_guide/zh/reference/keyboard' },
                { text: '命令行参考', link: '/user_guide/zh/reference/cli' },
              ],
            },
          ],
        },
      },
    },
    en: {
      label: 'English',
      lang: 'en-US',
      link: '/user_guide/en/',
      title: 'Neo Docs',
      description: 'Neo — Interactive Local Coding Agent',
      themeConfig: {
        nav: [
          { text: 'Guides', link: '/user_guide/en/guides/interaction', activeMatch: '/user_guide/en/guides/' },
          { text: 'Configuration', link: '/user_guide/en/configuration/config-files', activeMatch: '/user_guide/en/configuration/' },
          { text: 'Customization', link: '/user_guide/en/customization/mcp', activeMatch: '/user_guide/en/customization/' },
          { text: 'Reference', link: '/user_guide/en/reference/tools', activeMatch: '/user_guide/en/reference/' },
        ],
        sidebar: {
          '/user_guide/en/': [
            {
              text: 'Getting Started',
              items: [
                { text: 'Quickstart', link: '/user_guide/en/quickstart' },
              ],
            },
            {
              text: 'Guides',
              items: [
                { text: 'Interaction & Input', link: '/user_guide/en/guides/interaction' },
                { text: 'Sessions & Context', link: '/user_guide/en/guides/sessions' },
                { text: 'Using Goal Mode', link: '/user_guide/en/guides/goals' },
                { text: 'Using Plan Mode', link: '/user_guide/en/guides/plan-mode' },
                { text: 'Local Workflows', link: '/user_guide/en/guides/workflows' },
                { text: 'Common Use Cases', link: '/user_guide/en/guides/use-cases' },
              ],
            },
            {
              text: 'Configuration',
              items: [
                { text: 'Config Files', link: '/user_guide/en/configuration/config-files' },
                { text: 'Providers & Models', link: '/user_guide/en/configuration/providers' },
                { text: 'Permission Modes', link: '/user_guide/en/configuration/permissions' },
                { text: 'Data Locations', link: '/user_guide/en/configuration/data-locations' },
              ],
            },
            {
              text: 'Customization',
              items: [
                { text: 'Model Context Protocol', link: '/user_guide/en/customization/mcp' },
                { text: 'Agent Skills', link: '/user_guide/en/customization/skills' },
                { text: 'Agents & Sub-agents', link: '/user_guide/en/customization/agents' },
                { text: 'Custom Themes', link: '/user_guide/en/customization/themes' },
              ],
            },
            {
              text: 'Reference',
              items: [
                { text: 'Built-in Tools', link: '/user_guide/en/reference/tools' },
                { text: 'Slash Commands', link: '/user_guide/en/reference/slash-commands' },
                { text: 'Keyboard Shortcuts', link: '/user_guide/en/reference/keyboard' },
              ],
            },
          ],
        },
      },
    },
  },

  themeConfig: {
    socialLinks: [
      { icon: 'github', link: 'https://github.com/matrixheaven/neo' },
    ],
  },
})

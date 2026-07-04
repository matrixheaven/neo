import { defineConfig } from 'vitepress'

export default defineConfig({
  title: 'Neo',
  description: 'Neo — 交互式本地编程 Agent',

  head: [
    ['meta', { name: 'theme-color', content: '#C678DD' }],
  ],

  locales: {
    zh: {
      label: '简体中文',
      lang: 'zh-CN',
      link: '/zh/',
      title: 'Neo 文档',
      description: 'Neo — 交互式本地编程 Agent',
      themeConfig: {
        nav: [
          { text: '指南', link: '/zh/guides/interaction', activeMatch: '/zh/guides/' },
          { text: '配置', link: '/zh/configuration/config-files', activeMatch: '/zh/configuration/' },
          { text: '定制化', link: '/zh/customization/mcp', activeMatch: '/zh/customization/' },
          { text: '参考手册', link: '/zh/reference/tools', activeMatch: '/zh/reference/' },
        ],
        sidebar: {
          '/zh/': [
            {
              text: '快速开始',
              items: [
                { text: '快速上手', link: '/zh/quickstart' },
              ],
            },
            {
              text: '指南',
              items: [
                { text: '交互与输入', link: '/zh/guides/interaction' },
                { text: '会话与上下文', link: '/zh/guides/sessions' },
                { text: '使用目标模式', link: '/zh/guides/goals' },
                { text: '使用计划模式', link: '/zh/guides/plan-mode' },
                { text: '常见使用案例', link: '/zh/guides/use-cases' },
              ],
            },
            {
              text: '配置',
              items: [
                { text: '配置文件', link: '/zh/configuration/config-files' },
                { text: '平台与模型', link: '/zh/configuration/providers' },
                { text: '权限模式', link: '/zh/configuration/permissions' },
                { text: '数据路径', link: '/zh/configuration/data-locations' },
              ],
            },
            {
              text: '定制化',
              items: [
                { text: 'Model Context Protocol', link: '/zh/customization/mcp' },
                { text: 'Agent Skills', link: '/zh/customization/skills' },
                { text: 'Agent 与子 Agent', link: '/zh/customization/agents' },
                { text: '自定义主题', link: '/zh/customization/themes' },
              ],
            },
            {
              text: '参考手册',
              items: [
                { text: '内置工具', link: '/zh/reference/tools' },
                { text: '斜杠命令', link: '/zh/reference/slash-commands' },
                { text: '键盘快捷键', link: '/zh/reference/keyboard' },
              ],
            },
          ],
        },
      },
    },
    en: {
      label: 'English',
      lang: 'en-US',
      link: '/en/',
      title: 'Neo Docs',
      description: 'Neo — Interactive Local Coding Agent',
      themeConfig: {
        nav: [
          { text: 'Guides', link: '/en/guides/interaction', activeMatch: '/en/guides/' },
          { text: 'Configuration', link: '/en/configuration/config-files', activeMatch: '/en/configuration/' },
          { text: 'Customization', link: '/en/customization/mcp', activeMatch: '/en/customization/' },
          { text: 'Reference', link: '/en/reference/tools', activeMatch: '/en/reference/' },
        ],
        sidebar: {
          '/en/': [
            {
              text: 'Getting Started',
              items: [
                { text: 'Quickstart', link: '/en/quickstart' },
              ],
            },
            {
              text: 'Guides',
              items: [
                { text: 'Interaction & Input', link: '/en/guides/interaction' },
                { text: 'Sessions & Context', link: '/en/guides/sessions' },
                { text: 'Using Goal Mode', link: '/en/guides/goals' },
                { text: 'Using Plan Mode', link: '/en/guides/plan-mode' },
                { text: 'Common Use Cases', link: '/en/guides/use-cases' },
              ],
            },
            {
              text: 'Configuration',
              items: [
                { text: 'Config Files', link: '/en/configuration/config-files' },
                { text: 'Providers & Models', link: '/en/configuration/providers' },
                { text: 'Permission Modes', link: '/en/configuration/permissions' },
                { text: 'Data Locations', link: '/en/configuration/data-locations' },
              ],
            },
            {
              text: 'Customization',
              items: [
                { text: 'Model Context Protocol', link: '/en/customization/mcp' },
                { text: 'Agent Skills', link: '/en/customization/skills' },
                { text: 'Agents & Sub-agents', link: '/en/customization/agents' },
                { text: 'Custom Themes', link: '/en/customization/themes' },
              ],
            },
            {
              text: 'Reference',
              items: [
                { text: 'Built-in Tools', link: '/en/reference/tools' },
                { text: 'Slash Commands', link: '/en/reference/slash-commands' },
                { text: 'Keyboard Shortcuts', link: '/en/reference/keyboard' },
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

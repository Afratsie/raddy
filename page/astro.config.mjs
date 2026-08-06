// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

// Lightweight TextMate grammar for the Raddyfile (Caddyfile-like DSL), so
// `caddyfile` code blocks get real syntax highlighting instead of a txt
// fallback. Covers comments, strings, numbers, directive keywords,
// `{placeholder}` tokens, and block braces.
const caddyfileLang = {
	name: 'caddyfile',
	scopeName: 'source.caddyfile',
	patterns: [
		{ include: '#comment' },
		{ include: '#string' },
		{ include: '#number' },
		{ include: '#placeholder' },
		{ include: '#directive' },
		{ include: '#braces' },
	],
	repository: {
		comment: { match: /#.*$/, name: 'comment.line.number-sign.caddyfile' },
		string: { begin: /"/, end: /"/, name: 'string.quoted.double.caddyfile' },
		number: { match: /\b\d+\b/, name: 'constant.numeric.caddyfile' },
		placeholder: {
			match: /\{[a-z_]+}/,
			name: 'constant.other.placeholder.caddyfile',
		},
		directive: {
			match: /\b(?:reverse_proxy|file_server|redir|handle|root|encode|header_up|header_down|rate_limit|trusted_proxies|lb_policy|health_check|to|interval|timeout|consecutive_failures|consecutive_successes|log_level|acme_email|admin|snippet|import)\b/,
			name: 'keyword.control.caddyfile',
		},
		braces: { match: /[{}]/, name: 'punctuation.section.block.caddyfile' },
	},
};

// https://astro.build/config
export default defineConfig({
	site: 'https://chulingera2025.github.io',
	base: '/raddy/',
	integrations: [
		starlight({
			title: 'Raddy',
			description: 'A minimal high-performance reverse proxy gateway built on Cloudflare Pingora.',
			// English is the default (root) locale; Simplified Chinese lives
			// under /zh-CN/. Locale directories:
			//   src/content/docs/          → English (root)
			//   src/content/docs/zh-cn/    → 简体中文
			defaultLocale: 'root',
			locales: {
				root: { label: 'English', lang: 'en' },
				'zh-cn': { label: '简体中文', lang: 'zh-CN' },
			},
			social: [
				{ icon: 'github', label: 'GitHub', href: 'https://github.com/chulingera2025/raddy' },
			],
			editLink: {
				baseUrl: 'https://github.com/chulingera2025/raddy/edit/main/page',
			},
			expressiveCode: {
				shiki: { langs: [caddyfileLang] },
			},
			sidebar: [
				{
					label: 'Getting Started',
					translations: { 'zh-cn': '快速开始' },
					items: [
						{ label: 'Quick start', translations: { 'zh-cn': '快速上手' }, slug: 'quickstart' },
						{ label: 'Installation', translations: { 'zh-cn': '安装' }, slug: 'install' },
					],
				},
				{
					label: 'Guides',
					translations: { 'zh-cn': '指南' },
					items: [
						{ label: 'Serve static files', translations: { 'zh-cn': '静态托管' }, slug: 'guides/static-files' },
						{ label: 'Redirect HTTP → HTTPS', translations: { 'zh-cn': 'HTTP → HTTPS 重定向' }, slug: 'guides/http-to-https' },
						{ label: 'Proxy an API', translations: { 'zh-cn': '代理 API' }, slug: 'guides/api-proxy' },
					],
				},
				{
					label: 'Raddyfile',
					translations: { 'zh-cn': 'Raddyfile' },
					items: [
						{ label: 'Concepts', translations: { 'zh-cn': '核心概念' }, slug: 'config' },
						{ label: 'Directives', translations: { 'zh-cn': '指令参考' }, slug: 'config/directives' },
						{ label: 'Sites, ports & HTTPS', translations: { 'zh-cn': '站点 · 端口 · HTTPS' }, slug: 'config/sites' },
						{ label: 'Trusted proxies', translations: { 'zh-cn': '可信代理' }, slug: 'config/trusted-proxies' },
					],
				},
				{
					label: 'CLI & Operations',
					translations: { 'zh-cn': 'CLI 与运维' },
					items: [
						{ label: 'CLI reference', translations: { 'zh-cn': 'CLI 参考' }, slug: 'cli' },
						{ label: 'Metrics', translations: { 'zh-cn': '指标' }, slug: 'operations/metrics' },
						{ label: 'Access log', translations: { 'zh-cn': '访问日志' }, slug: 'operations/access-log' },
						{ label: 'Performance', translations: { 'zh-cn': '性能' }, slug: 'performance' },
					],
				},
			],
		}),
	],
});
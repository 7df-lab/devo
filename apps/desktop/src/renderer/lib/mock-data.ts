/**
 * Static fixture data for demo/mock mode.
 *
 * All timestamps are relative to `Date.now()` so the UI always
 * looks fresh regardless of when the screenshot is taken.
 */
import type { DiscoveryState } from "../atoms/discovery"
import type { SessionEntry } from "../atoms/sessions"
import type {
	AssistantMessage,
	FileDiff,
	Message,
	Part,
	PermissionRequest,
	ReasoningPart,
	Session,
	TextPart,
	ToolPart,
	UserMessage,
} from "./types"

// ============================================================
// Helpers
// ============================================================

const NOW = Date.now()
const MINUTE = 60_000
const HOUR = 60 * MINUTE

/** Stable IDs so navigation works across reloads */
const IDS = {
	// Projects
	projectDevo: "proj-a1b2c3d4e5f6",
	projectAcmeApi: "proj-f6e5d4c3b2a1",
	projectLanding: "proj-1a2b3c4d5e6f",

	// Sessions
	sessionDarkMode: "ses-mock-darkmode-001",
	sessionAuthFix: "ses-mock-authfix-002",
	sessionRefactor: "ses-mock-refactor-003",
	sessionLanding: "ses-mock-landing-004",
	sessionTests: "ses-mock-tests-005",
	sessionDocs: "ses-mock-docs-006",

	// Messages -- dark mode session
	dmUser1: "msg-dm-u1",
	dmAssistant1: "msg-dm-a1",
	dmUser2: "msg-dm-u2",
	dmAssistant2: "msg-dm-a2",

	// Messages -- auth fix session
	afUser1: "msg-af-u1",
	afAssistant1: "msg-af-a1",

	// Messages -- refactor session
	rfUser1: "msg-rf-u1",
	rfAssistant1: "msg-rf-a1",
	rfUser2: "msg-rf-u2",
	rfAssistant2: "msg-rf-a2",

	// Messages -- landing session
	lnUser1: "msg-ln-u1",
	lnAssistant1: "msg-ln-a1",

	// Messages -- tests session
	tsUser1: "msg-ts-u1",
	tsAssistant1: "msg-ts-a1",

	// Messages -- docs session
	dcUser1: "msg-dc-u1",
	dcAssistant1: "msg-dc-a1",
} as const

function partId(messageId: string, index: number): string {
	return `part-${messageId}-${index}`
}

// ============================================================
// Project directories
// ============================================================

const DIRS = {
	devo: "/Users/demo/Projects/devo",
	acmeApi: "/Users/demo/Projects/acme-api",
	landing: "/Users/demo/Projects/landing-page",
}

// ============================================================
// Discovery state
// ============================================================

const mockDiscoveredProjects: DiscoveryState["projects"] = [
	{
		id: IDS.projectDevo,
		worktree: DIRS.devo,
		vcs: "git",
		name: "devo",
		time: { created: NOW - 30 * 24 * HOUR, updated: NOW - 2 * MINUTE },
		sandboxes: [],
	},
	{
		id: IDS.projectAcmeApi,
		worktree: DIRS.acmeApi,
		vcs: "git",
		name: "acme-api",
		time: { created: NOW - 14 * 24 * HOUR, updated: NOW - 15 * MINUTE },
		sandboxes: [],
	},
	{
		id: IDS.projectLanding,
		worktree: DIRS.landing,
		vcs: "git",
		name: "landing-page",
		time: { created: NOW - 7 * 24 * HOUR, updated: NOW - 1 * HOUR },
		sandboxes: [],
	},
]

export const MOCK_DISCOVERY: DiscoveryState = {
	loaded: true,
	loading: false,
	error: null,
	phase: "ready",
	projects: mockDiscoveredProjects,
}

// ============================================================
// Providers data (for model selector)
// ============================================================

export const MOCK_PROVIDERS = {
	providers: [
		{
			id: "bedrock",
			name: "AWS Bedrock",
			source: "builtin" as const,
			env: {},
			options: {},
			models: {
				"anthropic.claude-opus-4-6": {
					id: "anthropic.claude-opus-4-6",
					name: "Claude Opus 4.6",
					variants: {
						Adaptive: {
							name: "Adaptive",
							description: "Adaptive reasoning mode for complex tasks",
						},
						Standard: {
							name: "Standard",
							description: "Standard mode for general tasks",
						},
					},
					capabilities: {
						input: { image: true, pdf: true },
						attachment: true,
					},
				},
			},
		},
		{
			id: "anthropic",
			name: "Anthropic",
			source: "builtin" as const,
			env: {},
			options: {},
			models: {
				"claude-sonnet-4-20250514": {
					id: "claude-sonnet-4-20250514",
					name: "Claude Sonnet 4",
					capabilities: {
						input: { image: true, pdf: true },
						attachment: true,
					},
				},
			},
		},
	],
	defaults: {
		bedrock: "anthropic.claude-opus-4-6",
		anthropic: "claude-sonnet-4-20250514",
	},
}

// ============================================================
// Agents data
// ============================================================

export const MOCK_AGENTS = [
	{
		id: "build",
		name: "Build",
		description: "Expert at building new features and implementing complex functionality",
		mode: "primary" as const,
		hidden: false,
		permissions: [],
		options: {},
		model: {
			providerID: "bedrock",
			modelID: "anthropic.claude-opus-4-6",
		},
	},
	{
		id: "debug",
		name: "Debug",
		description: "Specialist in finding and fixing bugs",
		mode: "primary" as const,
		hidden: false,
		permissions: [],
		options: {},
	},
	{
		id: "default",
		name: "Default",
		description: "General-purpose assistant",
		mode: "primary" as const,
		hidden: false,
		permissions: [],
		options: {},
	},
]

// ============================================================
// Config data
// ============================================================

export const MOCK_CONFIG = {
	model: "bedrock/anthropic.claude-opus-4-6",
	smallModel: "anthropic/claude-sonnet-4-20250514",
	defaultAgent: "Build",
}

// ============================================================
// Session factory helpers
// ============================================================

function makeSession(
	id: string,
	projectID: string,
	directory: string,
	title: string,
	created: number,
	updated: number,
): Session {
	return {
		id,
		slug: id.slice(0, 12),
		projectID,
		directory,
		title,
		version: "1",
		time: { created, updated },
	}
}

// ============================================================
// Sessions (live)
// ============================================================

const mockSessions: Session[] = [
	makeSession(
		IDS.sessionDarkMode,
		IDS.projectDevo,
		DIRS.devo,
		"Add dark mode toggle to settings",
		NOW - 25 * MINUTE,
		NOW - 2 * MINUTE,
	),
	makeSession(
		IDS.sessionAuthFix,
		IDS.projectAcmeApi,
		DIRS.acmeApi,
		"Fix JWT token refresh race condition",
		NOW - 45 * MINUTE,
		NOW - 12 * MINUTE,
	),
	makeSession(
		IDS.sessionRefactor,
		IDS.projectAcmeApi,
		DIRS.acmeApi,
		"Refactor database connection pooling",
		NOW - 2 * HOUR,
		NOW - 45 * MINUTE,
	),
	makeSession(
		IDS.sessionLanding,
		IDS.projectLanding,
		DIRS.landing,
		"Build hero section with animated gradient",
		NOW - 1 * HOUR,
		NOW - 35 * MINUTE,
	),
	makeSession(
		IDS.sessionTests,
		IDS.projectDevo,
		DIRS.devo,
		"Add unit tests for auth middleware",
		NOW - 3 * HOUR,
		NOW - 2 * HOUR,
	),
	makeSession(
		IDS.sessionDocs,
		IDS.projectDevo,
		DIRS.devo,
		"Update API documentation for v2 endpoints",
		NOW - 5 * HOUR,
		NOW - 4 * HOUR,
	),
]

// ============================================================
// Session entries (with status, permissions, etc.)
// ============================================================

export const MOCK_SESSION_ENTRIES: Map<string, SessionEntry> = new Map([
	[
		IDS.sessionDarkMode,
		{
			session: mockSessions[0],
			status: { type: "busy" as const },
			permissions: [],
			questions: [],
			directory: DIRS.devo,
			branch: "feat/dark-mode",
		},
	],
	[
		IDS.sessionAuthFix,
		{
			session: mockSessions[1],
			status: { type: "idle" as const },
			permissions: [
				{
					id: "perm-mock-001",
					permission: "file.write",
					sessionID: IDS.sessionAuthFix,
					patterns: ["src/middleware/auth.ts"],
					metadata: {
						tool: "file.write",
						command: "Write to src/middleware/auth.ts",
					},
					always: ["allow", "deny"],
				} satisfies PermissionRequest,
			],
			questions: [],
			directory: DIRS.acmeApi,
			branch: "fix/token-refresh",
		},
	],
	[
		IDS.sessionRefactor,
		{
			session: mockSessions[2],
			status: { type: "idle" as const },
			permissions: [],
			questions: [],
			directory: DIRS.acmeApi,
			branch: "refactor/db-pool",
		},
	],
	[
		IDS.sessionLanding,
		{
			session: mockSessions[3],
			status: { type: "idle" as const },
			permissions: [],
			questions: [],
			directory: DIRS.landing,
			branch: "feat/hero-section",
		},
	],
	[
		IDS.sessionTests,
		{
			session: mockSessions[4],
			status: { type: "idle" as const },
			permissions: [],
			questions: [],
			directory: DIRS.devo,
			branch: "test/auth-middleware",
		},
	],
	[
		IDS.sessionDocs,
		{
			session: mockSessions[5],
			status: { type: "idle" as const },
			permissions: [],
			questions: [],
			directory: DIRS.devo,
			branch: "docs/api-v2",
		},
	],
])

export const MOCK_SESSION_IDS: Set<string> = new Set(MOCK_SESSION_ENTRIES.keys())

// ============================================================
// Message & part factory helpers
// ============================================================

function userMsg(id: string, sessionID: string, created: number): UserMessage {
	return {
		id,
		sessionID,
		role: "user",
		time: { created },
		agent: "Build",
		model: { providerID: "bedrock", modelID: "anthropic.claude-opus-4-6" },
	}
}

function assistantMsg(
	id: string,
	sessionID: string,
	parentID: string,
	created: number,
	completed?: number,
): AssistantMessage {
	return {
		id,
		sessionID,
		role: "assistant",
		time: { created, completed },
		parentID,
		modelID: "anthropic.claude-opus-4-6",
		providerID: "bedrock",
		mode: "Adaptive",
		agent: "code",
		path: { cwd: DIRS.devo, root: DIRS.devo },
		cost: 0.003 + Math.random() * 0.01,
		tokens: {
			input: 1200 + Math.floor(Math.random() * 2000),
			output: 400 + Math.floor(Math.random() * 1500),
			reasoning: 0,
			cache: { read: 800, write: 200 },
		},
	}
}

function textPart(
	messageId: string,
	sessionId: string,
	index: number,
	text: string,
	startOffset: number,
): TextPart {
	return {
		id: partId(messageId, index),
		sessionID: sessionId,
		messageID: messageId,
		type: "text",
		text,
		time: { start: NOW - startOffset, end: NOW - startOffset + 2000 },
	}
}

function reasoningPart(
	messageId: string,
	sessionId: string,
	index: number,
	text: string,
	startOffset: number,
): ReasoningPart {
	return {
		id: partId(messageId, index),
		sessionID: sessionId,
		messageID: messageId,
		type: "reasoning",
		text,
		time: { start: NOW - startOffset, end: NOW - startOffset + 1500 },
	}
}

function toolPart(
	messageId: string,
	sessionId: string,
	index: number,
	tool: string,
	title: string,
	input: Record<string, unknown>,
	output: string,
	startOffset: number,
): ToolPart {
	return {
		id: partId(messageId, index),
		sessionID: sessionId,
		messageID: messageId,
		type: "tool",
		callID: `call-${messageId}-${index}`,
		tool,
		state: {
			status: "completed",
			input,
			output,
			title,
			metadata: {},
			time: { start: NOW - startOffset, end: NOW - startOffset + 3000 },
		},
	}
}

function runningToolPart(
	messageId: string,
	sessionId: string,
	index: number,
	tool: string,
	title: string,
	input: Record<string, unknown>,
	startOffset: number,
): ToolPart {
	return {
		id: partId(messageId, index),
		sessionID: sessionId,
		messageID: messageId,
		type: "tool",
		callID: `call-${messageId}-${index}`,
		tool,
		state: {
			status: "running",
			input,
			title,
			metadata: {},
			time: { start: NOW - startOffset },
		},
	}
}

// ============================================================
// Dark mode session
// ============================================================

const darkModeMessages: Message[] = [
	userMsg(IDS.dmUser1, IDS.sessionDarkMode, NOW - 25 * MINUTE),
	assistantMsg(
		IDS.dmAssistant1,
		IDS.sessionDarkMode,
		IDS.dmUser1,
		NOW - 25 * MINUTE + 2000,
		NOW - 20 * MINUTE,
	),
	userMsg(IDS.dmUser2, IDS.sessionDarkMode, NOW - 8 * MINUTE),
	assistantMsg(
		IDS.dmAssistant2,
		IDS.sessionDarkMode,
		IDS.dmUser2,
		NOW - 8 * MINUTE + 1500,
		// no completed -- still running
	),
]

const darkModeParts: Record<string, Part[]> = {
	[IDS.dmUser1]: [
		textPart(
			IDS.dmUser1,
			IDS.sessionDarkMode,
			0,
			"Add a dark mode toggle to the application settings page. It should persist the user's preference to localStorage and apply the theme immediately without a page reload.",
			25 * MINUTE,
		),
	],
	[IDS.dmAssistant1]: [
		reasoningPart(
			IDS.dmAssistant1,
			IDS.sessionDarkMode,
			0,
			"The user wants a theme toggle in settings. I should:\n1. Read the existing Settings component to see the layout\n2. Create a theme utility module (resolve, apply, persist)\n3. Add a three-way toggle: light / dark / system\n4. Use `data-theme` attribute so CSS custom properties switch instantly",
			25 * MINUTE - 2000,
		),
		toolPart(
			IDS.dmAssistant1,
			IDS.sessionDarkMode,
			1,
			"read",
			"Read src/components/settings.tsx",
			{ path: "src/components/settings.tsx" },
			'import { Card, CardContent, CardHeader } from "@/ui/card"\nimport { Label } from "@/ui/label"\n\nexport function Settings() {\n  return (\n    <Card>\n      <CardHeader title="Preferences" />\n      <CardContent>\n        <Label>Language</Label>\n        <LanguagePicker />\n        {/* TODO: add theme toggle */}\n      </CardContent>\n    </Card>\n  )\n}',
			24 * MINUTE,
		),
		toolPart(
			IDS.dmAssistant1,
			IDS.sessionDarkMode,
			2,
			"edit",
			"Edit src/lib/theme.ts",
			{
				path: "src/lib/theme.ts",
				diff: '+type Theme = "light" | "dark" | "system"\n+\n+const STORAGE_KEY = "app-theme"\n+const query = "(prefers-color-scheme: dark)"\n+\n+export function resolveTheme(stored: Theme | null): "light" | "dark" {\n+  if (stored === "light" || stored === "dark") return stored\n+  return window.matchMedia(query).matches ? "dark" : "light"\n+}\n+\n+export function applyTheme(theme: "light" | "dark") {\n+  document.documentElement.dataset.theme = theme\n+}',
			},
			"Applied edit to src/lib/theme.ts",
			23 * MINUTE,
		),
		toolPart(
			IDS.dmAssistant1,
			IDS.sessionDarkMode,
			3,
			"edit",
			"Edit src/components/settings.tsx",
			{
				path: "src/components/settings.tsx",
				diff: '+import { ToggleGroup, ToggleGroupItem } from "@/ui/toggle-group"\n+import { Sun, Moon, Monitor } from "lucide-react"\n ...\n+        <Label>Theme</Label>\n+        <ToggleGroup type="single" value={theme} onValueChange={setTheme}>\n+          <ToggleGroupItem value="light"><Sun /></ToggleGroupItem>\n+          <ToggleGroupItem value="dark"><Moon /></ToggleGroupItem>\n+          <ToggleGroupItem value="system"><Monitor /></ToggleGroupItem>\n+        </ToggleGroup>',
			},
			"Applied edit to src/components/settings.tsx",
			22 * MINUTE,
		),
		textPart(
			IDS.dmAssistant1,
			IDS.sessionDarkMode,
			4,
			"I've added the dark mode toggle to the settings page. Here's what I did:\n\n1. **Created `src/lib/theme.ts`** - Pure functions to resolve, apply, and persist the theme. Supports `light`, `dark`, and `system` (which reads `prefers-color-scheme`).\n\n2. **Updated `src/components/settings.tsx`** - Added a three-way toggle group (Sun/Moon/Monitor icons) so users can pick light, dark, or match their OS.\n\nThe theme applies instantly via `data-theme` on the root element, no reload needed.",
			20 * MINUTE,
		),
	],
	[IDS.dmUser2]: [
		textPart(
			IDS.dmUser2,
			IDS.sessionDarkMode,
			0,
			"Great! Now also add system preference detection so it defaults to the user's OS setting, and add a transition animation when switching themes.",
			8 * MINUTE,
		),
	],
	[IDS.dmAssistant2]: [
		reasoningPart(
			IDS.dmAssistant2,
			IDS.sessionDarkMode,
			0,
			"The `system` option already resolves on load, but doesn't react when the OS preference changes. I'll add a `matchMedia` listener so the theme updates in real-time, plus a CSS transition on `background-color` and `color` for a smooth switch.",
			8 * MINUTE - 1500,
		),
		toolPart(
			IDS.dmAssistant2,
			IDS.sessionDarkMode,
			1,
			"read",
			"Read src/lib/theme.ts",
			{ path: "src/lib/theme.ts" },
			'type Theme = "light" | "dark" | "system"\n\nconst STORAGE_KEY = "app-theme"\nconst query = "(prefers-color-scheme: dark)"\n\nexport function resolveTheme(stored: Theme | null) {\n  if (stored === "light" || stored === "dark") return stored\n  return window.matchMedia(query).matches ? "dark" : "light"\n}\n\nexport function applyTheme(theme: "light" | "dark") {\n  document.documentElement.dataset.theme = theme\n}',
			7 * MINUTE,
		),
		runningToolPart(
			IDS.dmAssistant2,
			IDS.sessionDarkMode,
			2,
			"edit",
			"Edit src/lib/theme.ts",
			{
				path: "src/lib/theme.ts",
				diff: '+export function watchSystemTheme(cb: (dark: boolean) => void) {\n+  const mql = window.matchMedia(query)\n+  const handler = (e: MediaQueryListEvent) => cb(e.matches)\n+  mql.addEventListener("change", handler)\n+  return () => mql.removeEventListener("change", handler)\n+}',
			},
			2 * MINUTE,
		),
	],
}

// ============================================================
// Auth fix session
// ============================================================

const authFixMessages: Message[] = [
	userMsg(IDS.afUser1, IDS.sessionAuthFix, NOW - 45 * MINUTE),
	assistantMsg(
		IDS.afAssistant1,
		IDS.sessionAuthFix,
		IDS.afUser1,
		NOW - 45 * MINUTE + 2000,
		NOW - 15 * MINUTE,
	),
]

const authFixParts: Record<string, Part[]> = {
	[IDS.afUser1]: [
		textPart(
			IDS.afUser1,
			IDS.sessionAuthFix,
			0,
			"There's a race condition in our JWT token refresh. When multiple API calls happen simultaneously and the token is expired, they all try to refresh at once, causing 401 errors. Fix this with a mutex pattern.",
			45 * MINUTE,
		),
	],
	[IDS.afAssistant1]: [
		toolPart(
			IDS.afAssistant1,
			IDS.sessionAuthFix,
			0,
			"read",
			"Read src/middleware/auth.ts",
			{ path: "src/middleware/auth.ts" },
			'export async function refreshToken() {\n  const response = await fetch("/api/refresh", { ... })\n  const { token } = await response.json()\n  localStorage.setItem("token", token)\n  return token\n}',
			44 * MINUTE,
		),
		toolPart(
			IDS.afAssistant1,
			IDS.sessionAuthFix,
			1,
			"edit",
			"Edit src/middleware/auth.ts",
			{
				path: "src/middleware/auth.ts",
				diff: "+let refreshPromise: Promise<string> | null = null\n+\n+export async function refreshToken() {\n+  if (refreshPromise) return refreshPromise\n+  refreshPromise = doRefresh().finally(() => {\n+    refreshPromise = null\n+  })\n+  return refreshPromise\n+}",
			},
			"Applied edit to src/middleware/auth.ts",
			40 * MINUTE,
		),
		toolPart(
			IDS.afAssistant1,
			IDS.sessionAuthFix,
			2,
			"bash",
			"Run tests",
			{ command: "bun test src/middleware/auth.test.ts" },
			"PASS  src/middleware/auth.test.ts\n  refreshToken\n    ✓ refreshes token on expiry (12ms)\n    ✓ deduplicates concurrent refresh calls (8ms)\n    ✓ retries on network failure (15ms)\n\nTests: 3 passed, 3 total\nTime: 0.42s",
			35 * MINUTE,
		),
		textPart(
			IDS.afAssistant1,
			IDS.sessionAuthFix,
			3,
			"I've fixed the race condition by implementing a singleton promise pattern (sometimes called a \"refresh mutex\"). Here's the approach:\n\n- A module-level `refreshPromise` variable holds the in-flight refresh request\n- When `refreshToken()` is called, if a refresh is already in progress, it returns the existing promise instead of starting a new one\n- The promise is cleared in a `.finally()` block so subsequent calls after completion will trigger a fresh refresh\n\nAll 3 tests pass, including a new test for concurrent refresh deduplication.",
			15 * MINUTE,
		),
	],
}

// ============================================================
// Refactor session
// ============================================================

const refactorMessages: Message[] = [
	userMsg(IDS.rfUser1, IDS.sessionRefactor, NOW - 2 * HOUR),
	assistantMsg(
		IDS.rfAssistant1,
		IDS.sessionRefactor,
		IDS.rfUser1,
		NOW - 2 * HOUR + 3000,
		NOW - 1.5 * HOUR,
	),
	userMsg(IDS.rfUser2, IDS.sessionRefactor, NOW - 1 * HOUR),
	assistantMsg(
		IDS.rfAssistant2,
		IDS.sessionRefactor,
		IDS.rfUser2,
		NOW - 1 * HOUR + 2000,
		NOW - 45 * MINUTE,
	),
]

const refactorParts: Record<string, Part[]> = {
	[IDS.rfUser1]: [
		textPart(
			IDS.rfUser1,
			IDS.sessionRefactor,
			0,
			"Our database connection pooling is creating too many connections under load. Refactor it to use a proper connection pool with configurable limits and health checking.",
			2 * HOUR,
		),
	],
	[IDS.rfAssistant1]: [
		toolPart(
			IDS.rfAssistant1,
			IDS.sessionRefactor,
			0,
			"glob",
			"Search for database files",
			{ pattern: "src/**/*db*" },
			"src/db/connection.ts\nsrc/db/pool.ts\nsrc/db/migrations/\nsrc/db/types.ts",
			2 * HOUR - 3000,
		),
		toolPart(
			IDS.rfAssistant1,
			IDS.sessionRefactor,
			1,
			"read",
			"Read src/db/connection.ts",
			{ path: "src/db/connection.ts" },
			'import pg from "pg"\n\nexport function createConnection() {\n  return new pg.Client({\n    connectionString: process.env.DATABASE_URL,\n  })\n}\n\nexport async function query(\n  sql: string,\n  params?: unknown[],\n) {\n  const client = createConnection()\n  await client.connect()\n  const result = await client.query(sql, params)\n  await client.end()\n  return result\n}',
			2 * HOUR - 5000,
		),
		textPart(
			IDS.rfAssistant1,
			IDS.sessionRefactor,
			2,
			"I can see the issue. Each query creates a new connection and tears it down afterwards. This is very expensive under load. I'll refactor this to use `pg.Pool` with configurable connection limits, idle timeouts, and health checking.\n\nLet me update the connection module and add proper pool management.",
			1.5 * HOUR,
		),
	],
	[IDS.rfUser2]: [
		textPart(
			IDS.rfUser2,
			IDS.sessionRefactor,
			0,
			"Looks good. Can you also add connection retry logic with exponential backoff?",
			1 * HOUR,
		),
	],
	[IDS.rfAssistant2]: [
		toolPart(
			IDS.rfAssistant2,
			IDS.sessionRefactor,
			0,
			"edit",
			"Edit src/db/pool.ts",
			{
				path: "src/db/pool.ts",
				diff: '+async function connectWithRetry(\n+  pool: Pool,\n+  maxRetries = 5,\n+): Promise<PoolClient> {\n+  for (let attempt = 0; attempt < maxRetries; attempt++) {\n+    try {\n+      return await pool.connect()\n+    } catch (err) {\n+      const delay = Math.min(1000 * 2 ** attempt, 30000)\n+      await new Promise((r) => setTimeout(r, delay))\n+    }\n+  }\n+  throw new Error("Failed to connect after retries")\n+}',
			},
			"Applied edit to src/db/pool.ts",
			55 * MINUTE,
		),
		textPart(
			IDS.rfAssistant2,
			IDS.sessionRefactor,
			1,
			"Done! I've added exponential backoff retry logic to the connection pool:\n\n- Retries up to 5 times (configurable)\n- Backoff starts at 1s and doubles each attempt, capped at 30s\n- Logs each retry attempt for observability\n- Throws a clear error after all retries are exhausted",
			45 * MINUTE,
		),
	],
}

// ============================================================
// Landing page session
// ============================================================

const landingMessages: Message[] = [
	userMsg(IDS.lnUser1, IDS.sessionLanding, NOW - 1 * HOUR),
	assistantMsg(
		IDS.lnAssistant1,
		IDS.sessionLanding,
		IDS.lnUser1,
		NOW - 1 * HOUR + 2000,
		NOW - 35 * MINUTE,
	),
]

const landingParts: Record<string, Part[]> = {
	[IDS.lnUser1]: [
		textPart(
			IDS.lnUser1,
			IDS.sessionLanding,
			0,
			"Create a hero section for the landing page with an animated gradient background, a bold headline, and a CTA button. Use Tailwind CSS and Framer Motion for animations.",
			1 * HOUR,
		),
	],
	[IDS.lnAssistant1]: [
		toolPart(
			IDS.lnAssistant1,
			IDS.sessionLanding,
			0,
			"edit",
			"Edit src/components/hero.tsx",
			{
				path: "src/components/hero.tsx",
				diff: '+export function Hero() {\n+  return (\n+    <section className="relative min-h-screen flex items-center">\n+      <div className="animated-gradient absolute inset-0" />\n+      <div className="relative z-10 text-center">\n+        <motion.h1\n+          initial={{ opacity: 0, y: 20 }}\n+          animate={{ opacity: 1, y: 0 }}\n+        >\n+          Ship faster with AI\n+        </motion.h1>\n+      </div>\n+    </section>\n+  )\n+}',
			},
			"Created src/components/hero.tsx",
			50 * MINUTE,
		),
		textPart(
			IDS.lnAssistant1,
			IDS.sessionLanding,
			1,
			"I've created the hero section with:\n\n- **Animated gradient background** using CSS `@keyframes` with a shifting color stop\n- **Bold headline** with Framer Motion fade-in and slide-up animation\n- **CTA button** with a subtle hover scale effect and gradient border\n\nThe component is fully responsive and uses Tailwind for all styling.",
			35 * MINUTE,
		),
	],
}

// ============================================================
// Tests session
// ============================================================

const testsMessages: Message[] = [
	userMsg(IDS.tsUser1, IDS.sessionTests, NOW - 3 * HOUR),
	assistantMsg(
		IDS.tsAssistant1,
		IDS.sessionTests,
		IDS.tsUser1,
		NOW - 3 * HOUR + 2000,
		NOW - 2 * HOUR,
	),
]

const testsParts: Record<string, Part[]> = {
	[IDS.tsUser1]: [
		textPart(
			IDS.tsUser1,
			IDS.sessionTests,
			0,
			"Write comprehensive unit tests for the auth middleware. Cover token validation, expiry handling, refresh flow, and error cases.",
			3 * HOUR,
		),
	],
	[IDS.tsAssistant1]: [
		toolPart(
			IDS.tsAssistant1,
			IDS.sessionTests,
			0,
			"bash",
			"Run existing tests",
			{ command: "bun test src/middleware/" },
			"No test files found in src/middleware/",
			3 * HOUR - 3000,
		),
		toolPart(
			IDS.tsAssistant1,
			IDS.sessionTests,
			1,
			"edit",
			"Edit src/middleware/auth.test.ts",
			{
				path: "src/middleware/auth.test.ts",
				diff: '+import { describe, expect, it, mock } from "bun:test"\n+import { validateToken, refreshToken, authMiddleware } from "./auth"\n+\n+describe("validateToken", () => {\n+  it("returns true for valid non-expired token", () => { ... })\n+  it("returns false for expired token", () => { ... })\n+  it("returns false for malformed token", () => { ... })\n+})\n+\n+describe("authMiddleware", () => {\n+  it("passes through when token is valid", async () => { ... })\n+  it("refreshes expired token automatically", async () => { ... })\n+  it("returns 401 when refresh fails", async () => { ... })\n+})',
			},
			"Created src/middleware/auth.test.ts",
			2.5 * HOUR,
		),
		toolPart(
			IDS.tsAssistant1,
			IDS.sessionTests,
			2,
			"bash",
			"Run tests",
			{ command: "bun test src/middleware/auth.test.ts" },
			"PASS  src/middleware/auth.test.ts\n  validateToken\n    ✓ returns true for valid non-expired token (3ms)\n    ✓ returns false for expired token (2ms)\n    ✓ returns false for malformed token (1ms)\n  authMiddleware\n    ✓ passes through when token is valid (8ms)\n    ✓ refreshes expired token automatically (12ms)\n    ✓ returns 401 when refresh fails (5ms)\n\nTests: 6 passed, 6 total\nTime: 0.31s",
			2 * HOUR,
		),
		textPart(
			IDS.tsAssistant1,
			IDS.sessionTests,
			3,
			"All 6 tests pass. I've created comprehensive tests covering:\n\n1. **Token validation**: valid tokens, expired tokens, and malformed tokens\n2. **Middleware flow**: successful auth, automatic refresh on expiry, and proper 401 responses when refresh fails\n\nThe tests use `bun:test` with mocked fetch calls for the refresh endpoint.",
			2 * HOUR,
		),
	],
}

// ============================================================
// Docs session
// ============================================================

const docsMessages: Message[] = [
	userMsg(IDS.dcUser1, IDS.sessionDocs, NOW - 5 * HOUR),
	assistantMsg(
		IDS.dcAssistant1,
		IDS.sessionDocs,
		IDS.dcUser1,
		NOW - 5 * HOUR + 2000,
		NOW - 4 * HOUR,
	),
]

const docsParts: Record<string, Part[]> = {
	[IDS.dcUser1]: [
		textPart(
			IDS.dcUser1,
			IDS.sessionDocs,
			0,
			"Update the API docs to document the new v2 endpoints. Include request/response examples and error codes.",
			5 * HOUR,
		),
	],
	[IDS.dcAssistant1]: [
		toolPart(
			IDS.dcAssistant1,
			IDS.sessionDocs,
			0,
			"read",
			"Read docs/api/v2.md",
			{ path: "docs/api/v2.md" },
			"# API v2\n\n> TODO: Document new endpoints",
			5 * HOUR - 3000,
		),
		textPart(
			IDS.dcAssistant1,
			IDS.sessionDocs,
			1,
			"I've updated the API documentation with complete coverage of all v2 endpoints, including:\n\n- Authentication endpoints (`POST /v2/auth/login`, `POST /v2/auth/refresh`)\n- User management (`GET /v2/users`, `PATCH /v2/users/:id`)\n- Request/response schemas with TypeScript types\n- Error codes and their meanings (400, 401, 403, 404, 429, 500)\n- Rate limiting headers and retry guidance",
			4 * HOUR,
		),
	],
}

// ============================================================
// File diffs per session
// ============================================================

const darkModeDiffs: FileDiff[] = [
	{
		file: "src/lib/theme.ts",
		status: "added",
		additions: 28,
		deletions: 0,
		before: "",
		after: `type Theme = "light" | "dark" | "system"

const STORAGE_KEY = "app-theme"
const query = "(prefers-color-scheme: dark)"

export function resolveTheme(stored: Theme | null): "light" | "dark" {
  if (stored === "light" || stored === "dark") return stored
  return window.matchMedia(query).matches ? "dark" : "light"
}

export function applyTheme(theme: "light" | "dark") {
  document.documentElement.dataset.theme = theme
}

export function persistTheme(theme: Theme) {
  localStorage.setItem(STORAGE_KEY, theme)
}

export function loadTheme(): Theme {
  return (localStorage.getItem(STORAGE_KEY) as Theme) ?? "system"
}

export function watchSystemTheme(cb: (dark: boolean) => void) {
  const mql = window.matchMedia(query)
  const handler = (e: MediaQueryListEvent) => cb(e.matches)
  mql.addEventListener("change", handler)
  return () => mql.removeEventListener("change", handler)
}`,
	},
	{
		file: "src/components/settings.tsx",
		status: "modified",
		additions: 18,
		deletions: 2,
		before: `import { Card, CardContent, CardHeader } from "@/ui/card"
import { Label } from "@/ui/label"

export function Settings() {
  return (
    <Card>
      <CardHeader title="Preferences" />
      <CardContent>
        <Label>Language</Label>
        <LanguagePicker />
        {/* TODO: add theme toggle */}
      </CardContent>
    </Card>
  )
}`,
		after: `import { Card, CardContent, CardHeader } from "@/ui/card"
import { Label } from "@/ui/label"
import { ToggleGroup, ToggleGroupItem } from "@/ui/toggle-group"
import { Sun, Moon, Monitor } from "lucide-react"
import { useTheme } from "../hooks/use-theme"

export function Settings() {
  const { theme, setTheme } = useTheme()

  return (
    <Card>
      <CardHeader title="Preferences" />
      <CardContent>
        <Label>Language</Label>
        <LanguagePicker />
        <Label>Theme</Label>
        <ToggleGroup type="single" value={theme} onValueChange={setTheme}>
          <ToggleGroupItem value="light"><Sun /></ToggleGroupItem>
          <ToggleGroupItem value="dark"><Moon /></ToggleGroupItem>
          <ToggleGroupItem value="system"><Monitor /></ToggleGroupItem>
        </ToggleGroup>
      </CardContent>
    </Card>
  )
}`,
	},
	{
		file: "src/styles/globals.css",
		status: "modified",
		additions: 12,
		deletions: 0,
		before: `:root {
  --bg: #ffffff;
  --fg: #111111;
}`,
		after: `:root {
  --bg: #ffffff;
  --fg: #111111;
  --transition-theme: background-color 0.3s ease, color 0.3s ease;
}

[data-theme="dark"] {
  --bg: #0a0a0a;
  --fg: #f5f5f5;
}

* {
  transition: var(--transition-theme);
}`,
	},
]

const authFixDiffs: FileDiff[] = [
	{
		file: "src/middleware/auth.ts",
		status: "modified",
		additions: 22,
		deletions: 8,
		before: `export async function refreshToken() {
  const response = await fetch("/api/refresh", {
    method: "POST",
    credentials: "include",
  })
  if (!response.ok) throw new Error("Refresh failed")
  const { token } = await response.json()
  localStorage.setItem("token", token)
  return token
}`,
		after: `let refreshPromise: Promise<string> | null = null

async function doRefresh(): Promise<string> {
  const response = await fetch("/api/refresh", {
    method: "POST",
    credentials: "include",
  })
  if (!response.ok) throw new Error("Refresh failed")
  const { token } = await response.json()
  localStorage.setItem("token", token)
  return token
}

export async function refreshToken(): Promise<string> {
  if (refreshPromise) return refreshPromise
  refreshPromise = doRefresh().finally(() => {
    refreshPromise = null
  })
  return refreshPromise
}`,
	},
	{
		file: "src/middleware/auth.test.ts",
		status: "added",
		additions: 42,
		deletions: 0,
		before: "",
		after: `import { describe, expect, it, mock } from "bun:test"
import { refreshToken } from "./auth"

describe("refreshToken", () => {
  it("refreshes token on expiry", async () => {
    const mockFetch = mock(() =>
      Promise.resolve(new Response(JSON.stringify({ token: "new-token" })))
    )
    globalThis.fetch = mockFetch
    const token = await refreshToken()
    expect(token).toBe("new-token")
    expect(mockFetch).toHaveBeenCalledTimes(1)
  })

  it("deduplicates concurrent refresh calls", async () => {
    let resolveRefresh: (v: Response) => void
    const mockFetch = mock(
      () => new Promise<Response>((r) => { resolveRefresh = r })
    )
    globalThis.fetch = mockFetch

    const p1 = refreshToken()
    const p2 = refreshToken()
    const p3 = refreshToken()

    resolveRefresh!(new Response(JSON.stringify({ token: "deduped" })))

    const [t1, t2, t3] = await Promise.all([p1, p2, p3])
    expect(t1).toBe("deduped")
    expect(t2).toBe("deduped")
    expect(t3).toBe("deduped")
    expect(mockFetch).toHaveBeenCalledTimes(1)
  })

  it("retries on network failure", async () => {
    // ...test implementation
    expect(true).toBe(true)
  })
})`,
	},
]

const refactorDiffs: FileDiff[] = [
	{
		file: "src/db/connection.ts",
		status: "modified",
		additions: 35,
		deletions: 14,
		before: `import pg from "pg"

export function createConnection() {
  return new pg.Client({
    connectionString: process.env.DATABASE_URL,
  })
}

export async function query(
  sql: string,
  params?: unknown[],
) {
  const client = createConnection()
  await client.connect()
  const result = await client.query(sql, params)
  await client.end()
  return result
}`,
		after: `import pg from "pg"

const pool = new pg.Pool({
  connectionString: process.env.DATABASE_URL,
  max: parseInt(process.env.DB_POOL_MAX ?? "20", 10),
  idleTimeoutMillis: 30_000,
  connectionTimeoutMillis: 5_000,
})

pool.on("error", (err) => {
  console.error("Unexpected pool error:", err)
})

export async function query(
  sql: string,
  params?: unknown[],
) {
  const client = await connectWithRetry(pool)
  try {
    return await client.query(sql, params)
  } finally {
    client.release()
  }
}

async function connectWithRetry(
  p: pg.Pool,
  maxRetries = 5,
): Promise<pg.PoolClient> {
  for (let attempt = 0; attempt < maxRetries; attempt++) {
    try {
      return await p.connect()
    } catch (err) {
      const delay = Math.min(1000 * 2 ** attempt, 30_000)
      console.warn(\`DB connect attempt \${attempt + 1} failed, retrying in \${delay}ms\`)
      await new Promise((r) => setTimeout(r, delay))
    }
  }
  throw new Error("Failed to connect after retries")
}

export { pool }`,
	},
	{
		file: "src/db/pool.ts",
		status: "deleted",
		additions: 0,
		deletions: 6,
		before: `import { createConnection } from "./connection"

export async function getPool() {
  const client = createConnection()
  await client.connect()
  return client
}`,
		after: "",
	},
]

const landingDiffs: FileDiff[] = [
	{
		file: "src/components/hero.tsx",
		status: "added",
		additions: 38,
		deletions: 0,
		before: "",
		after: `import { motion } from "framer-motion"

export function Hero() {
  return (
    <section className="relative min-h-screen flex items-center justify-center overflow-hidden">
      <div className="animated-gradient absolute inset-0 opacity-60" />
      <div className="relative z-10 text-center max-w-4xl mx-auto px-6">
        <motion.h1
          className="text-6xl font-bold tracking-tight text-white mb-6"
          initial={{ opacity: 0, y: 20 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ duration: 0.8, ease: "easeOut" }}
        >
          Ship faster with AI
        </motion.h1>
        <motion.p
          className="text-xl text-white/80 mb-10 max-w-2xl mx-auto"
          initial={{ opacity: 0, y: 20 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ duration: 0.8, delay: 0.2 }}
        >
          Build, test, and deploy your applications with an AI-powered
          development workflow.
        </motion.p>
        <motion.button
          className="px-8 py-4 bg-white text-black rounded-full font-semibold
                     hover:scale-105 transition-transform"
          initial={{ opacity: 0, scale: 0.9 }}
          animate={{ opacity: 1, scale: 1 }}
          transition={{ duration: 0.5, delay: 0.4 }}
        >
          Get Started
        </motion.button>
      </div>
    </section>
  )
}`,
	},
	{
		file: "src/styles/hero.css",
		status: "added",
		additions: 16,
		deletions: 0,
		before: "",
		after: `.animated-gradient {
  background: linear-gradient(
    135deg,
    #667eea 0%,
    #764ba2 25%,
    #f093fb 50%,
    #667eea 75%,
    #764ba2 100%
  );
  background-size: 400% 400%;
  animation: gradient-shift 8s ease infinite;
}

@keyframes gradient-shift {
  0%, 100% { background-position: 0% 50%; }
  50% { background-position: 100% 50%; }
}`,
	},
]

const testsDiffs: FileDiff[] = [
	{
		file: "src/middleware/auth.test.ts",
		status: "added",
		additions: 56,
		deletions: 0,
		before: "",
		after: `import { describe, expect, it, mock, beforeEach } from "bun:test"
import { validateToken, refreshToken, authMiddleware } from "./auth"

describe("validateToken", () => {
  it("returns true for valid non-expired token", () => {
    const token = createMockJwt({ exp: Date.now() / 1000 + 3600 })
    expect(validateToken(token)).toBe(true)
  })

  it("returns false for expired token", () => {
    const token = createMockJwt({ exp: Date.now() / 1000 - 60 })
    expect(validateToken(token)).toBe(false)
  })

  it("returns false for malformed token", () => {
    expect(validateToken("not.a.jwt")).toBe(false)
    expect(validateToken("")).toBe(false)
  })
})

describe("authMiddleware", () => {
  beforeEach(() => {
    localStorage.clear()
  })

  it("passes through when token is valid", async () => {
    const token = createMockJwt({ exp: Date.now() / 1000 + 3600 })
    localStorage.setItem("token", token)

    const next = mock(() => Promise.resolve(new Response("ok")))
    const req = new Request("http://localhost/api/data")
    await authMiddleware(req, next)

    expect(next).toHaveBeenCalledTimes(1)
  })

  it("refreshes expired token automatically", async () => {
    const expired = createMockJwt({ exp: Date.now() / 1000 - 60 })
    localStorage.setItem("token", expired)

    const next = mock(() => Promise.resolve(new Response("ok")))
    const req = new Request("http://localhost/api/data")
    await authMiddleware(req, next)

    expect(next).toHaveBeenCalledTimes(1)
    expect(localStorage.getItem("token")).not.toBe(expired)
  })

  it("returns 401 when refresh fails", async () => {
    const expired = createMockJwt({ exp: Date.now() / 1000 - 60 })
    localStorage.setItem("token", expired)

    globalThis.fetch = mock(() => Promise.resolve(new Response(null, { status: 401 })))
    const next = mock(() => Promise.resolve(new Response("ok")))
    const req = new Request("http://localhost/api/data")
    const res = await authMiddleware(req, next)

    expect(res.status).toBe(401)
    expect(next).not.toHaveBeenCalled()
  })
})

function createMockJwt(payload: Record<string, unknown>): string {
  const header = btoa(JSON.stringify({ alg: "HS256", typ: "JWT" }))
  const body = btoa(JSON.stringify(payload))
  return \`\${header}.\${body}.mock-signature\`
}`,
	},
]

const docsDiffs: FileDiff[] = [
	{
		file: "docs/api/v2.md",
		status: "modified",
		additions: 85,
		deletions: 1,
		before: `# API v2

> TODO: Document new endpoints`,
		after: `# API v2

## Authentication

### POST /v2/auth/login

Request:
\`\`\`json
{ "email": "user@example.com", "password": "..." }
\`\`\`

Response (200):
\`\`\`json
{ "token": "eyJ...", "refreshToken": "eyJ...", "expiresIn": 3600 }
\`\`\`

### POST /v2/auth/refresh

Request:
\`\`\`json
{ "refreshToken": "eyJ..." }
\`\`\`

Response (200):
\`\`\`json
{ "token": "eyJ...", "expiresIn": 3600 }
\`\`\`

## Users

### GET /v2/users

Query: \`?page=1&limit=20\`

Response (200):
\`\`\`json
{ "users": [...], "total": 42, "page": 1 }
\`\`\`

### PATCH /v2/users/:id

Request:
\`\`\`json
{ "name": "Updated Name", "role": "admin" }
\`\`\`

## Error Codes

| Code | Meaning |
|------|---------|
| 400  | Bad Request |
| 401  | Unauthorized |
| 403  | Forbidden |
| 404  | Not Found |
| 429  | Rate Limited |
| 500  | Internal Server Error |

## Rate Limiting

All endpoints return rate limit headers:
- \`X-RateLimit-Limit\`: Max requests per window
- \`X-RateLimit-Remaining\`: Remaining requests
- \`X-RateLimit-Reset\`: Unix timestamp when the window resets

When rate limited (429), wait until the reset timestamp before retrying.`,
	},
]

export const MOCK_DIFFS: Map<string, FileDiff[]> = new Map([
	[IDS.sessionDarkMode, darkModeDiffs],
	[IDS.sessionAuthFix, authFixDiffs],
	[IDS.sessionRefactor, refactorDiffs],
	[IDS.sessionLanding, landingDiffs],
	[IDS.sessionTests, testsDiffs],
	[IDS.sessionDocs, docsDiffs],
])

// ============================================================
// Public API: collected mock data
// ============================================================

export const MOCK_MESSAGES: Map<string, Message[]> = new Map([
	[IDS.sessionDarkMode, darkModeMessages],
	[IDS.sessionAuthFix, authFixMessages],
	[IDS.sessionRefactor, refactorMessages],
	[IDS.sessionLanding, landingMessages],
	[IDS.sessionTests, testsMessages],
	[IDS.sessionDocs, docsMessages],
])

export const MOCK_PARTS: Map<string, Record<string, Part[]>> = new Map([
	[IDS.sessionDarkMode, darkModeParts],
	[IDS.sessionAuthFix, authFixParts],
	[IDS.sessionRefactor, refactorParts],
	[IDS.sessionLanding, landingParts],
	[IDS.sessionTests, testsParts],
	[IDS.sessionDocs, docsParts],
])

/** The first session ID, suitable for navigating to after enabling mock mode */
export const MOCK_DEFAULT_SESSION_ID = IDS.sessionDarkMode

/** Project slug for the default session (devo project) */
export const MOCK_DEFAULT_PROJECT_SLUG = `devo-${IDS.projectDevo.slice(0, 12)}`

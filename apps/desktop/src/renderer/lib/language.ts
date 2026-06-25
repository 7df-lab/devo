/**
 * Shiki BundledLanguage string type alias.
 * We use a string-based type here to avoid requiring shiki as a direct dependency
 * in the desktop app — the actual Shiki types are consumed inside the UI package.
 */
type Lang = string

/**
 * Maps file extensions to Shiki BundledLanguage identifiers.
 * Used to provide syntax highlighting for tool card outputs.
 */
const EXTENSION_MAP: Record<string, Lang> = {
	// Web
	js: "javascript",
	mjs: "javascript",
	cjs: "javascript",
	jsx: "jsx",
	ts: "typescript",
	mts: "typescript",
	cts: "typescript",
	tsx: "tsx",
	vue: "vue",
	svelte: "svelte",
	astro: "astro",
	html: "html",
	htm: "html",
	css: "css",
	scss: "scss",
	sass: "sass",
	less: "less",
	// Data / Config
	json: "json",
	jsonc: "jsonc",
	json5: "json5",
	yaml: "yaml",
	yml: "yaml",
	toml: "toml",
	xml: "xml",
	csv: "csv",
	env: "dotenv",
	// Markdown / Docs
	md: "markdown",
	mdx: "mdx",
	// Shell
	sh: "bash",
	bash: "bash",
	zsh: "bash",
	fish: "fish",
	ps1: "powershell",
	bat: "bat",
	cmd: "bat",
	// Languages
	py: "python",
	rb: "ruby",
	rs: "rust",
	go: "go",
	java: "java",
	kt: "kotlin",
	kts: "kotlin",
	scala: "scala",
	swift: "swift",
	c: "c",
	h: "c",
	cpp: "cpp",
	cc: "cpp",
	cxx: "cpp",
	hpp: "cpp",
	cs: "csharp",
	php: "php",
	r: "r",
	lua: "lua",
	zig: "zig",
	nim: "nim",
	dart: "dart",
	ex: "elixir",
	exs: "elixir",
	erl: "erlang",
	hs: "haskell",
	clj: "clojure",
	lisp: "lisp",
	ml: "ocaml",
	fs: "fsharp",
	jl: "julia",
	// DevOps / Infra
	dockerfile: "dockerfile",
	tf: "terraform",
	hcl: "hcl",
	nix: "nix",
	// Config files
	ini: "ini",
	conf: "ini",
	cfg: "ini",
	properties: "java",
	gradle: "groovy",
	groovy: "groovy",
	// Query languages
	sql: "sql",
	graphql: "graphql",
	gql: "graphql",
	prisma: "prisma",
	// Other
	diff: "diff",
	patch: "diff",
	log: "log",
	txt: "plaintext",
	makefile: "makefile",
	cmake: "cmake",
	// Biome / Prettier
	biome: "jsonc",
}

/**
 * Special filenames that map to a specific language regardless of extension.
 */
const FILENAME_MAP: Record<string, Lang> = {
	Dockerfile: "dockerfile",
	Makefile: "makefile",
	CMakeLists: "cmake",
	Justfile: "just",
	Vagrantfile: "ruby",
	Gemfile: "ruby",
	Rakefile: "ruby",
	Procfile: "bash",
	".gitignore": "gitignore",
	".dockerignore": "gitignore",
	".env": "dotenv",
	".env.local": "dotenv",
	".env.development": "dotenv",
	".env.production": "dotenv",
	"tsconfig.json": "jsonc",
	"biome.json": "jsonc",
	"biome.jsonc": "jsonc",
}

/**
 * Detect a Shiki language from a file path.
 * Returns undefined if the language cannot be determined.
 */
export function detectLanguage(filePath: string | undefined): Lang | undefined {
	if (!filePath) return undefined

	// Check filename-based matches first
	const fileName = filePath.split("/").pop() ?? filePath
	const filenameMatch = FILENAME_MAP[fileName]
	if (filenameMatch) return filenameMatch

	// Check by extension
	const ext = fileName.includes(".") ? fileName.split(".").pop()?.toLowerCase() : undefined
	if (ext) {
		return EXTENSION_MAP[ext]
	}

	return undefined
}

/**
 * Try to detect if a string is JSON and return "json" language,
 * or if it looks like a diff/patch, return "diff".
 * Otherwise return undefined.
 */
export function detectContentLanguage(content: string): Lang | undefined {
	const trimmed = content.trimStart()

	// JSON detection: starts with { or [
	if (trimmed.startsWith("{") || trimmed.startsWith("[")) {
		try {
			JSON.parse(content)
			return "json"
		} catch {
			// Not valid JSON, could be JS object — don't highlight
		}
	}

	// Diff / patch detection
	if (trimmed.startsWith("---") || trimmed.startsWith("diff --git") || trimmed.startsWith("@@")) {
		return "diff"
	}

	return undefined
}

/**
 * Pretty-print a JSON string if it's valid JSON and not already formatted.
 * Returns the original string unchanged if it's not valid JSON.
 */
export function prettyPrintJson(content: string): string {
	const trimmed = content.trimStart()
	if (!trimmed.startsWith("{") && !trimmed.startsWith("[")) return content
	try {
		const parsed = JSON.parse(content)
		return JSON.stringify(parsed, null, 2)
	} catch {
		return content
	}
}

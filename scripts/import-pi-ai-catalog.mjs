/**
 * Import @earendil-works/pi-ai static MODELS into crates/core/providers.json.
 *
 * - Runtime catalog stays server-owned JSON (L2-DES-MODEL-002 / RLM-001).
 * - Default input is the committed snapshot (no network, no sibling prime-agent).
 * - Refresh snapshot only with PI_AI_MODELS=…/models.generated.ts or --refresh-snapshot.
 * - Unsupported pi-ai wire APIs are skipped (Bedrock / Vertex / Mistral / Azure).
 * - OAuth helpers are never imported.
 * - After convert, Devo-local templates are restored from the previous providers.json:
 *   kimi, poolside, zhipu, qwen, tencent, ollama.
 *
 * Usage:
 *   node scripts/import-pi-ai-catalog.mjs
 *   node --experimental-strip-types scripts/import-pi-ai-catalog.mjs --refresh-snapshot
 *   PI_AI_MODELS=C:\path\to\models.generated.ts node --experimental-strip-types scripts/import-pi-ai-catalog.mjs
 */
import { mkdirSync, readFileSync, writeFileSync, existsSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const root = resolve(__dirname, "..");
const outPath = join(root, "crates", "core", "providers.json");
const snapshotDir = join(root, "crates", "core", "catalog");
const snapshotPath = join(snapshotDir, "pi-ai-models.snapshot.json");

const API_MAP = {
  "openai-completions": "openai_chat_completions",
  "openai-responses": "openai_responses",
  "openai-codex-responses": "openai_responses",
  "anthropic-messages": "anthropic_messages",
  "google-generative-ai": "google_generative_ai",
};

/** Providers kept from the prior Devo catalog after every import. */
const DEVO_LOCAL_PROVIDER_IDS = [
  "kimi",
  "poolside",
  "zhipu",
  "qwen",
  "tencent",
  "ollama",
];

const PROVIDER_META = {
  anthropic: { name: "Anthropic", description: "Claude models via Anthropic API" },
  "openai-codex": {
    name: "ChatGPT",
    description: "ChatGPT Plus/Pro via Codex",
  },
  openai: { name: "OpenAI", description: "OpenAI API models" },
  "github-copilot": { name: "GitHub Copilot", description: "GitHub Copilot models" },
  xai: { name: "xAI", description: "xAI Grok models" },
  google: { name: "Google", description: "Google AI Studio / Gemini" },
  deepseek: { name: "DeepSeek", description: "DeepSeek models" },
  zai: { name: "Z.ai", description: "International GLM API" },
  openrouter: { name: "OpenRouter", description: "OpenRouter model gateway" },
  "vercel-ai-gateway": {
    name: "Vercel AI Gateway",
    description: "Vercel AI Gateway models",
  },
  "prime-inference": {
    name: "Hosted Inference",
    description: "Hosted inference models",
  },
  groq: { name: "Groq", description: "Groq models" },
  moonshotai: { name: "Moonshot AI", description: "Moonshot / Kimi API" },
  "moonshotai-cn": {
    name: "Moonshot AI (CN)",
    description: "Moonshot China endpoint",
  },
  "kimi-coding": { name: "Kimi Coding", description: "Kimi coding models" },
  minimax: { name: "MiniMax", description: "MiniMax models" },
  "minimax-cn": { name: "MiniMax (CN)", description: "MiniMax China endpoint" },
  xiaomi: { name: "Xiaomi", description: "Xiaomi MiMo models" },
  cerebras: { name: "Cerebras", description: "Cerebras models" },
  fireworks: { name: "Fireworks", description: "Fireworks AI models" },
  huggingface: { name: "Hugging Face", description: "Hugging Face Inference" },
  opencode: { name: "OpenCode", description: "OpenCode catalog models" },
  "opencode-go": { name: "OpenCode Go", description: "OpenCode Go models" },
  "cloudflare-ai-gateway": {
    name: "Cloudflare AI Gateway",
    description: "Cloudflare AI Gateway",
  },
  "cloudflare-workers-ai": {
    name: "Cloudflare Workers AI",
    description: "Cloudflare Workers AI",
  },
  "xiaomi-token-plan-ams": {
    name: "Xiaomi Token Plan (AMS)",
    description: "Xiaomi token plan AMS",
  },
  "xiaomi-token-plan-cn": {
    name: "Xiaomi Token Plan (CN)",
    description: "Xiaomi token plan CN",
  },
  "xiaomi-token-plan-sgp": {
    name: "Xiaomi Token Plan (SGP)",
    description: "Xiaomi token plan SGP",
  },
};

const refreshSnapshot =
  process.argv.includes("--refresh-snapshot") || Boolean(process.env.PI_AI_MODELS);

function titleCaseProvider(id) {
  return id
    .split("-")
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
    .join(" ");
}

function thinkingLevelsFromMap(map, reasoning) {
  if (!reasoning) return undefined;
  const keys = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];
  if (!map || typeof map !== "object") {
    return { levels: ["off", "low", "medium", "high"] };
  }
  const levels = keys.filter((level) => {
    const mapped = map[level];
    if (mapped === null) return false;
    if (level === "xhigh" || level === "max") return mapped !== undefined;
    return true;
  });
  return levels.length > 0 ? { levels } : { levels: ["off", "low", "medium", "high"] };
}

function defaultEffort(levels, reasoning) {
  if (!reasoning || !levels?.levels?.length) return undefined;
  const preferred = ["high", "medium", "low", "xhigh", "max"];
  for (const level of preferred) {
    if (levels.levels.includes(level)) return level;
  }
  return levels.levels.find((level) => level !== "off") ?? levels.levels[0];
}

async function loadModelsFromGeneratedTs(path) {
  const mod = await import(pathToFileURL(path).href);
  if (!mod.MODELS) {
    throw new Error(`No MODELS export in ${path}`);
  }
  return mod.MODELS;
}

function loadModelsFromSnapshot() {
  if (!existsSync(snapshotPath)) {
    throw new Error(
      `Missing committed snapshot at ${snapshotPath}. ` +
        `Refresh once with PI_AI_MODELS=…/models.generated.ts or --refresh-snapshot.`,
    );
  }
  return JSON.parse(readFileSync(snapshotPath, "utf8"));
}

async function resolveRefreshSource() {
  const envPath = process.env.PI_AI_MODELS;
  const candidates = [
    envPath,
    join(root, "apps", "tui", "lib", "ai", "src", "models.generated.ts"),
    join(root, "..", "prime-agent", "packages", "ai", "src", "models.generated.ts"),
    join(root, "vendor", "pi-ai", "src", "models.generated.ts"),
  ].filter(Boolean);

  for (const candidate of candidates) {
    if (!existsSync(candidate)) continue;
    return candidate;
  }
  throw new Error(
    "Snapshot refresh requires PI_AI_MODELS pointing at models.generated.ts " +
      "(or a sibling prime-agent / vendor/pi-ai checkout).",
  );
}

async function loadModels() {
  if (refreshSnapshot) {
    const source = await resolveRefreshSource();
    const models = await loadModelsFromGeneratedTs(source);
    console.log(`refreshing snapshot from ${source}`);
    return { models, source, writeSnapshot: true };
  }

  const models = loadModelsFromSnapshot();
  console.log(`loaded MODELS from snapshot ${snapshotPath}`);
  return { models, source: snapshotPath, writeSnapshot: false };
}

/**
 * Provider wire_api = majority mapped API among kept models.
 * Each model sets wire_api when it differs from that majority (mixed providers).
 */
function convertModels(MODELS) {
  const staged = new Map();
  let kept = 0;
  let skipped = 0;

  for (const [providerId, modelMap] of Object.entries(MODELS)) {
    for (const model of Object.values(modelMap)) {
      const wireApi = API_MAP[model.api];
      if (!wireApi) {
        skipped += 1;
        continue;
      }
      kept += 1;
      if (!staged.has(providerId)) {
        staged.set(providerId, {
          counts: new Map(),
          models: [],
          baseUrl: model.baseUrl ?? null,
        });
      }
      const bucket = staged.get(providerId);
      bucket.counts.set(wireApi, (bucket.counts.get(wireApi) ?? 0) + 1);
      if (!bucket.baseUrl && model.baseUrl) bucket.baseUrl = model.baseUrl;
      bucket.models.push(model);
    }
  }

  const providers = {};
  for (const [providerId, bucket] of staged.entries()) {
    let majorityApi = null;
    let majorityCount = -1;
    for (const [api, count] of bucket.counts.entries()) {
      if (count > majorityCount) {
        majorityApi = api;
        majorityCount = count;
      }
    }

    const meta = PROVIDER_META[providerId] ?? {
      name: titleCaseProvider(providerId),
      description: `${titleCaseProvider(providerId)} models (from pi-ai static catalog)`,
    };
    const entry = {
      name: meta.name,
      description: meta.description,
      ...(bucket.baseUrl ? { base_url: bucket.baseUrl } : {}),
      wire_api: majorityApi,
      models: {},
    };

    for (const model of bucket.models) {
      const wireApi = API_MAP[model.api];
      const levels = thinkingLevelsFromMap(model.thinkingLevelMap, model.reasoning);
      const converted = {
        name: model.name,
        channel: entry.name,
        ...(model.reasoning
          ? {
              reasoning: true,
              reasoning_capability: levels,
              default_reasoning_effort: defaultEffort(levels, true),
              ...(model.thinkingLevelMap
                ? { thinking_level_map: model.thinkingLevelMap }
                : {}),
            }
          : {}),
        context_window: model.contextWindow,
        ...(typeof model.maxTokens === "number" ? { max_tokens: model.maxTokens } : {}),
        input_modalities: Array.isArray(model.input) ? model.input : ["text"],
        ...(model.featured ? { priority: 50 } : { priority: 10 }),
        ...(model.cost ? { cost: model.cost } : {}),
      };
      if (wireApi !== majorityApi) {
        converted.wire_api = wireApi;
      }
      entry.models[model.id] = converted;
    }

    providers[providerId] = entry;
  }

  return { providers, kept, skipped };
}

function mergeLocalProviders(providers, previous) {
  const prevProviders = previous?.provider ?? {};
  for (const id of DEVO_LOCAL_PROVIDER_IDS) {
    if (prevProviders[id]) {
      providers[id] = prevProviders[id];
    }
  }
  return providers;
}

function pickDefaultModel(providers) {
  const preferred = [
    "openai/gpt-5.5",
    "anthropic/claude-sonnet-4-6",
    "anthropic/claude-sonnet-4-5",
    "openai-codex/gpt-5.5",
    "kimi/kimi-k3",
  ];
  for (const slug of preferred) {
    const [providerId, modelId] = slug.split("/");
    if (providers[providerId]?.models?.[modelId]) return slug;
  }
  let best = null;
  let bestPriority = -Infinity;
  for (const [providerId, provider] of Object.entries(providers)) {
    for (const [modelId, model] of Object.entries(provider.models ?? {})) {
      const priority = model.priority ?? 0;
      if (priority > bestPriority) {
        bestPriority = priority;
        best = `${providerId}/${modelId}`;
      }
    }
  }
  return best;
}

const previous = existsSync(outPath)
  ? JSON.parse(readFileSync(outPath, "utf8"))
  : { provider: {} };

const { models: MODELS, source, writeSnapshot } = await loadModels();

if (writeSnapshot) {
  mkdirSync(snapshotDir, { recursive: true });
  writeFileSync(snapshotPath, JSON.stringify(MODELS, null, 2) + "\n", "utf8");
  console.log(`wrote snapshot ${snapshotPath}`);
}

const { providers, kept, skipped } = convertModels(MODELS);
mergeLocalProviders(providers, previous);

const sorted = Object.fromEntries(
  Object.entries(providers).sort(([a], [b]) => a.localeCompare(b)),
);

const defaultModel = pickDefaultModel(sorted);
const output = {
  ...(defaultModel ? { model: defaultModel } : {}),
  provider: sorted,
};

writeFileSync(outPath, JSON.stringify(output, null, 2) + "\n", "utf8");
console.log(
  `wrote ${outPath} (kept=${kept} skipped=${skipped} providers=${Object.keys(sorted).length} default=${defaultModel} source=${source})`,
);

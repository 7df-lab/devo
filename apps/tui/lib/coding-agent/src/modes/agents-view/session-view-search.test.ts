/**
 * Regression tests for the agents-view search corpus split.
 *
 * Live defect (R45): the fuzzy fallback ran over the FULL field join, so a
 * session-id fragment like "01a0c620" found a low-gap subsequence across
 * field boundaries (score ~3 against a gate of 25) and matched nearly every
 * session — the filter looked inert. Identity fields must match exactly;
 * only prose (titles, messages, summaries) gets typo tolerance.
 */

import { test } from "node:test";
import assert from "node:assert/strict";
import { matchesSearchText, matchSearchText, parseSearchQuery } from "./session-view-search.js";
import {
	filterUnifiedSessions,
	reconcileUnifiedSessions,
	type UnifiedSessionRecord,
} from "./agents-view-state.js";
import type { SessionSummary } from "../daemon/daemon-session-list.js";

function summary(partial: Partial<SessionSummary>): SessionSummary {
	return {
		id: partial.id ?? "ses_base",
		lifecycle: "live",
		activity: "idle",
		isSessionActive: false,
		sessionId: partial.id ?? "ses_base",
		cwd: partial.cwd ?? "/tmp/r25",
		isStreaming: false,
		isCompacting: false,
		attachedClients: 0,
		messageCount: 0,
		sessionActions: { queuedCount: 0, steering: [], followUps: [] },
		...partial,
	} as SessionSummary;
}

/** Same-day ids share the "ses_01a0c6" prefix, like real server-issued ids. */
const ID_FORK = "ses_01a0c620e3e67ea385fff398748b4f4e";
const ID_SIBLING = "ses_01a0c6095f2b78a082982b56cf06f152";
const ID_ROOT = "ses_01a0c48f77f171c0bcfd2dc786d4e28b";

const corpus = {
	// Everything the sibling record knows: two ids + title + cwd.
	searchText: `${ID_SIBLING} ${ID_SIBLING} Spawning sleeper subagent for idle marker task /tmp/r25`,
	fuzzyText: "Spawning sleeper subagent for idle marker task",
};

test("session-id fragment does not fuzzy-match across corpus fields", () => {
	// Substring over the full join: only the record owning the id matches.
	assert.equal(matchesSearchText(corpus.searchText, corpus.fuzzyText, "01a0c620"), false);
	assert.equal(
		matchesSearchText(`${ID_FORK} ${ID_FORK} ${corpus.fuzzyText}`, corpus.fuzzyText, "01a0c620"),
		true,
	);
});

test("fuzzy typo tolerance still works for prose", () => {
	assert.equal(matchesSearchText(corpus.searchText, corpus.fuzzyText, "sleepr subagent"), true);
	assert.equal(matchesSearchText(corpus.searchText, "", "sleepr subagent"), false);
});

test("phrase and regex queries still run over the full join", () => {
	assert.equal(matchesSearchText(corpus.searchText, corpus.fuzzyText, '"sleeper subagent"'), true);
	assert.equal(matchesSearchText(corpus.searchText, corpus.fuzzyText, '"task /tmp/r25"'), true);
	assert.equal(matchesSearchText(corpus.searchText, corpus.fuzzyText, "re:task\\s+/tmp"), true);
	assert.equal(matchesSearchText(corpus.searchText, corpus.fuzzyText, "re:absentzz"), false);
});

test("matchSearchText scores substring position on the exact corpus", () => {
	const parsed = parseSearchQuery("sleeper");
	const result = matchSearchText(corpus.searchText, corpus.fuzzyText, parsed);
	assert.equal(result.matches, true);
	assert.equal(result.score, corpus.searchText.toLowerCase().indexOf("sleeper") * 0.1);
});

function summariesWithSharedIdPrefix(): SessionSummary[] {
	return [
		summary({
			id: ID_ROOT,
			sessionId: ID_ROOT,
			activeSessionId: ID_ROOT,
			sessionName: "Spawning sleeper subagent for idle marker task",
		}),
		summary({
			id: ID_FORK,
			sessionId: ID_FORK,
			activeSessionId: ID_FORK,
			sessionName: "Forked rewind branch",
		}),
		summary({
			id: ID_SIBLING,
			sessionId: ID_SIBLING,
			activeSessionId: ID_SIBLING,
			sessionName: "Sibling spawned earlier",
			parentSessionId: ID_ROOT,
			parentActiveSessionId: ID_ROOT,
			runtimeKind: "subagent",
		}),
	];
}

test("filterUnifiedSessions narrows an id search to the owning record plus ancestors", () => {
	const records = reconcileUnifiedSessions(summariesWithSharedIdPrefix(), []);
	const filtered = filterUnifiedSessions(records, (record) =>
		matchesSearchText(record.searchableText, record.searchableFuzzyText, "01a0c620"),
	);
	const ids = filtered.map((record) => record.daemon?.sessionId);
	// Before the fix, the shared same-day id prefix let fuzzy match every
	// record; now only the fork matches and only its (absent) ancestors ride.
	assert.deepEqual(ids, [ID_FORK]);
});

test("filterUnifiedSessions keeps ancestors of a matching subagent", () => {
	const records = reconcileUnifiedSessions(summariesWithSharedIdPrefix(), []);
	const filtered = filterUnifiedSessions(records, (record) =>
		matchesSearchText(record.searchableText, record.searchableFuzzyText, "sibling spawned"),
	);
	const ids = filtered.map((record) => record.daemon?.sessionId);
	assert.deepEqual(ids, [ID_ROOT, ID_SIBLING]);
});

test("reconcile keeps ids out of the fuzzy corpus and in the exact corpus", () => {
	const records: UnifiedSessionRecord[] = reconcileUnifiedSessions(summariesWithSharedIdPrefix(), []);
	const fork = records.find((record) => record.daemon?.sessionId === ID_FORK);
	assert.ok(fork);
	assert.equal(fork.searchableText.includes(ID_FORK), true);
	assert.equal(fork.searchableFuzzyText.includes(ID_FORK), false);
	assert.equal(fork.searchableFuzzyText.includes("Forked rewind branch"), true);
});

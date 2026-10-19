/**
 * Trace: shell "$(...)" / "$var" pairs must not lex as inline math.
 *
 * Live symptom (round 56): a user message pasting
 *   for i in $(seq 1 200); do echo line-$i; done
 * rendered as "for i in (seq 1 200); do echo line-i; done" — the two dollars
 * paired up as an inline-math span, the delimiters were dropped and the body
 * pushed through latexToUnicode.
 */

import assert from "node:assert/strict";
import test from "node:test";
import { type MarkdownTheme, Markdown } from "../lib/tui/src/components/markdown.js";

const identity = (text: string): string => text;

const IDENTITY_THEME: MarkdownTheme = {
	heading: identity,
	link: identity,
	linkUrl: identity,
	code: identity,
	codeBlock: identity,
	codeBlockBorder: identity,
	quote: identity,
	quoteBorder: identity,
	hr: identity,
	listBullet: identity,
	bold: identity,
	italic: identity,
	strikethrough: identity,
	underline: identity,
};

function renderText(text: string): string {
	const component = new Markdown(text, 0, 0, IDENTITY_THEME);
	return component
		.render(200)
		.join("\n")
		// Drop ANSI control sequences so assertions see plain text.
		.replace(/\x1b\[[0-9;]*[A-Za-z]/g, "");
}

test("shell command substitution and variables stay literal", () => {
	const rendered = renderText("for i in $(seq 1 200); do echo line-$i; done");
	assert.ok(rendered.includes("$(seq 1 200)"), rendered);
	assert.ok(rendered.includes("line-$i"), rendered);
});

test("env-var pairs like $HOME and $PATH stay literal", () => {
	const rendered = renderText("copy $HOME/a to $PATH/b now");
	assert.ok(rendered.includes("$HOME/a"), rendered);
	assert.ok(rendered.includes("$PATH/b"), rendered);
});

test("prose dollar amounts stay literal", () => {
	const rendered = renderText("it costs between $5 and $10 today");
	assert.ok(rendered.includes("between $5 and $10"), rendered);
});

test("latex inline math still converts", () => {
	const rendered = renderText("the angle $\\alpha$ grows");
	assert.ok(rendered.includes("α"), rendered);
	assert.ok(!rendered.includes("$"), rendered);
});

test("math followed by punctuation still converts", () => {
	const rendered = renderText("(see $\\alpha$)");
	assert.ok(rendered.includes("α"), rendered);
});

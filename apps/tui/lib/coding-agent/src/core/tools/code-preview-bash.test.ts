import assert from "node:assert/strict";
import { test } from "node:test";
import { previewIpythonCode } from "./code-preview.js";

test("rlm.bash previews match bare bash and cell magic commands", () => {
	const expected = { language: "bash", text: "printf QA_BASH_1012" };
	for (const code of [
		'await bash("printf QA_BASH_1012")',
		'await rlm.bash("printf QA_BASH_1012")',
		'output = await rlm.bash("printf QA_BASH_1012")\nprint(output)',
		'import rlm\noutput = await rlm.bash("""\nprintf QA_BASH_1012\n""")',
		'await rlm.bash(r"printf QA_BASH_1012", timeout=1000)',
		"%%bash\nprintf QA_BASH_1012",
	]) {
		assert.deepEqual(previewIpythonCode(code), expected, code);
	}
});

test("rlm.bash expressions and unrelated bash methods keep a Python preview", () => {
	for (const code of [
		'await rlm.bash("printf " + marker)',
		'await rlm.bash(f"printf {marker}")',
		'await other.bash("printf QA_BASH_1012")',
		'print("await rlm.bash(\\\"printf QA_BASH_1012\\\")")',
	]) {
		assert.equal(previewIpythonCode(code).language, "python", code);
	}
});

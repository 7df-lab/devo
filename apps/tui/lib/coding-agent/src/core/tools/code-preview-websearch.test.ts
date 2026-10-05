import assert from "node:assert/strict";
import { test } from "node:test";
import { previewIpythonCode } from "./code-preview.js";

test("previewIpythonCode extracts websearch query on collapsed line", () => {
	assert.deepEqual(previewIpythonCode("await websearch.run('devo coding agent', num_results=1)"), {
		language: "python",
		text: "websearch 'devo coding agent'",
	});
	assert.deepEqual(previewIpythonCode("print(await websearch.run('nested query'))"), {
		language: "python",
		text: "websearch 'nested query'",
	});
	assert.deepEqual(previewIpythonCode('await websearch.run("""long query""")'), {
		language: "python",
		text: "websearch 'long query'",
	});
});

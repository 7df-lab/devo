// Offline gh stand-in: never invokes GitHub or a network client.
import assert from "node:assert/strict";
import { readFileSync, writeFileSync } from "node:fs";
import { join, basename } from "node:path";
const args = process.argv.slice(2);
const home = process.env.DEVO_SHARE_SMOKE_DIR;
assert.ok(home);
if (args.join(" ") === "auth status") process.exit(0);
assert.deepEqual(args.slice(0, 3), ["gist", "create", "--public=false"]);
assert.equal(args.length, 4);
assert.equal(basename(args[3]), "session.html");
assert.notEqual(args[3], join(home, "session.html"));
assert.ok(readFileSync(args[3], "utf8").includes("share-seed"));
writeFileSync(join(home, "share-upload.json"), JSON.stringify({ path: args[3] }));
console.log("https://gist.github.com/smoke/offline-fixture");

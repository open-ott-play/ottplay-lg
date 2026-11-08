"use strict";
// The same public cases are consumed by the Rust QuickJS adapter's tests.
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const { createHash } = require("node:crypto");
const root = path.resolve(__dirname, "..");
const vendor = path.join(root, "vendor");
const manifest = JSON.parse(fs.readFileSync(path.join(vendor, "ottplay-core.manifest.json")));
assert.equal(manifest.name, "ottplay-shared-core");
assert.deepEqual(fs.readFileSync(path.join(root, "licenses/ottplay-core.txt")),
    fs.readFileSync(path.join(vendor, "ottplay-core.LICENSE.txt")), "packaged core license");
for (const name of ["ottplay-core.js", "ottplay-core.LICENSE.txt"]) {
    const bytes = fs.readFileSync(path.join(vendor, name));
    assert.equal(bytes.length, manifest.artifacts[name].bytes, name);
    assert.equal(createHash("sha256").update(bytes).digest("hex"), manifest.artifacts[name].sha256, name);
}
const context = vm.createContext({});
vm.runInContext(fs.readFileSync(path.join(vendor, "ottplay-core.js"), "utf8"), context, { timeout: 5000 });
const cases = JSON.parse(fs.readFileSync(path.join(root, "rust-modules/shared-core/tests/fixtures/seek.json")));
assert(cases.length >= 25);
for (const item of cases) {
    const run = () => context.OttPlayCore.playbackSeekPlan(item.target, item.request);
    if (item.error) assert.throws(run, undefined, item.name);
    else assert.deepEqual(JSON.parse(JSON.stringify(run())), item.expected, item.name);
}
console.log(`PASS canonical shared-core receipt and ${cases.length} seek cases in Node`);

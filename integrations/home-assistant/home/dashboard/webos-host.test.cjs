"use strict";
const test = require("node:test"), assert = require("node:assert/strict"), platform = require("./webos-host.js");
function fixture() {
    const calls = [], timers = new Map(); let serial = 0;
    function Palm() { calls.push(this); }
    Palm.prototype.call = function (uri, body) { this.uri = uri; this.body = body; };
    Palm.prototype.cancel = function () { this.cancelled = true; };
    const host = { PalmServiceBridge: Palm }, time = { set(fn) { const id = ++serial; timers.set(id, fn); return id; }, clear(id) { timers.delete(id); } };
    return { host, calls, timers, api: platform.create(host, time) };
}
test("Palm reply is bounded, parsed once, fail-closed and safe against late callbacks", () => {
    for (const raw of ["not JSON", " ".repeat(4097), "[]", "null", '{"returnValue":false}', '{}']) {
        const f = fixture(); let success = 0, failure = 0;
        f.api.probe("router", 443, () => { success++; }, () => { failure++; });
        const callback = f.calls[0].onservicecallback; callback(raw); callback('{"returnValue":true}');
        assert.equal(success, 0); assert.equal(failure, 1); assert.equal(f.calls[0].cancelled, true);
    }
    const f = fixture(); let count = 0;
    f.api.probe("router", 443, result => { count++; assert.equal(result.ms, 3); }, () => assert.fail("Unexpected failure"));
    const callback = f.calls[0].onservicecallback;
    callback('{"returnValue":true,"status":"ok","ms":3}'); callback('{"returnValue":true}'); assert.equal(count, 1);
});
test("probe never accepts arbitrary endpoints or ports", () => {
    const f = fixture(); let failure = 0;
    f.api.probe("http://attacker", 443, () => assert.fail(), () => { failure++; });
    f.api.probe("router", 22, () => assert.fail(), () => { failure++; });
    assert.equal(failure, 2); assert.equal(f.calls.length, 0);
});
test("constant native close is single-flight, bounded, cancellable and never retries", () => {
    const f = fixture(); let result;
    assert.equal(f.api.installClose(), true); assert.equal(f.calls.length, 0);
    assert.equal(f.host.OTTHomeHost.close(ok => { result = ok; }), true); assert.equal(f.host.OTTHomeHost.close(), false);
    assert.equal(f.calls.length, 1); assert.equal(f.calls[0].uri, "luna://com.webos.applicationManager/launch"); assert.deepEqual(JSON.parse(f.calls[0].body), { id: "com.openottplay.player.native" });
    const late = f.calls[0].onservicecallback;
    [...f.timers.values()][0](); assert.equal(result, false); assert.equal(f.calls[0].cancelled, true); assert.equal(f.timers.size, 0);
    late('{"returnValue":true}'); assert.equal(result, false); assert.equal(f.calls.length, 1);
    const injected = { close() {}, launch() {} }, original = injected.close;
    assert.equal(platform.create({ PalmServiceBridge: f.host.PalmServiceBridge, OTTHomeHost: injected }).installClose(), false); assert.equal(injected.close, original);
    assert.equal(platform.create({}).installClose(), false);
});

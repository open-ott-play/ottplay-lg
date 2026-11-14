"use strict";
const test = require("node:test");
const assert = require("node:assert/strict");
const api = require("./core.js");
function fixture() {
    let now = 0, seq = 0;
    const timers = new Map(), requests = [], states = [];
    const clock = { now: () => now, set(fn, ms) { const id = ++seq; timers.set(id, { fn, at: now + ms }); return id; }, clear(id) { timers.delete(id); } };
    function advance(ms) {
        const end = now + ms;
        while (true) { const entries = [...timers].filter(([, t]) => t.at <= end).sort((a, b) => a[1].at - b[1].at); if (!entries.length) break; const [id, t] = entries[0]; now = t.at; timers.delete(id); t.fn(); }
        now = end;
    }
    function transport(request, done) { const item = { request, done, aborted: false }; requests.push(item); return () => { item.aborted = true; }; }
    return { clock, requests, states, advance, transport, options: { clock, transport, onState: state => states.push(state) }, timers };
}
function snapshot() { return { version: 1, entities: [{ id: "temp", label: "Room", state: "20", unit: "°C", kind: "temperature" }], actions: [{ id: "light", label: "Lights" }], apps: [{ id: "hdmi1", label: "HDMI 1", kind: "hdmi" }] }; }
function answer(item, value = snapshot()) { item.done(null, { text: JSON.stringify(value) }); }
test("gateway origin restrictions require explicit trusted LAN HTTP", () => {
    assert.equal(api.baseUrl("https://home.local/"), "https://home.local");
    for (const url of ["http://192.168.1.5:8080", "http://home.local"]) { assert.equal(api.baseUrl(url), null); assert.equal(api.baseUrl(url, true), url); }
    for (const url of ["https://user:password@host", "https://host/key", "https://host?token=x", "javascript:x", "http://example.com", "http://8.8.8.8"]) assert.equal(api.baseUrl(url, true), null);
});
test("visible-only status is single-flight, authenticated and backs off failures", () => {
    const f = fixture(), client = api.createClient(f.options);
    client.configure("https://home.local", "client-key-0000000000000000000000000000"); assert.equal(f.requests.length, 0);
    client.visible(true); client.refresh(); assert.equal(f.requests.length, 1);
    assert.equal(f.requests[0].request.headers.Authorization, "Bearer client-key-0000000000000000000000000000");
    answer(f.requests[0]); f.advance(14999); assert.equal(f.requests.length, 1); f.advance(1); assert.equal(f.requests.length, 2);
    f.requests[1].done("HTTP_ERROR"); assert.equal(f.states.at(-1).status, "unavailable"); assert.equal(f.states.at(-1).model, undefined);
    f.advance(29999); assert.equal(f.requests.length, 2); f.advance(1); assert.equal(f.requests.length, 3);
    client.visible(false); assert.equal(f.requests[2].aborted, true); answer(f.requests[2]); f.advance(120000); assert.equal(f.requests.length, 3);
});
test("only snapshot allowlisted action/app IDs can be posted, no retry on failed action", () => {
    const f = fixture(), client = api.createClient(f.options);
    client.visible(true); client.configure("https://home.local", "client-key-0000000000000000000000000000"); answer(f.requests[0]);
    assert.equal(client.action("unapproved"), false); assert.equal(client.action("hdmi1"), true); assert.equal(client.action("light"), false);
    const action = f.requests[1].request;
    assert.equal(action.method, "POST"); assert.equal(action.url, "https://home.local/api/action"); assert.equal(action.body, '{"id":"hdmi1"}');
    f.requests[1].done("TIMEOUT"); f.advance(30000); assert.equal(f.requests[2].request.method, undefined);
    client.cancel(); assert.equal(f.requests[2].aborted, true);
});
test("invalid snapshots fail closed and reconfiguration ignores stale callbacks", () => {
    const f = fixture(), client = api.createClient(f.options);
    client.visible(true); client.configure("https://home.local", "old-key-0000000000000000000000000000"); client.configure("https://next.local", "new-key-0000000000000000000000000000");
    answer(f.requests[0]); assert.equal(f.states.at(-1).status, "loading");
    const malformed = snapshot(); malformed.apps.push({ id: "web", label: "Bad URL", kind: "url" }); answer(f.requests[1], malformed);
    assert.equal(f.states.at(-1).status, "unavailable"); assert.equal(client.action("hdmi1"), false);
});
test("unavailable gateway snapshot never authorizes actions and HA JWT cannot be configured as client key", () => {
    const f = fixture(), client = api.createClient(f.options); client.visible(true);
    assert.equal(client.configure("https://home.local", "eyJheader.eyJpayload.signature"), false); assert.equal(f.requests.length, 0);
    client.configure("https://home.local", "client-key-0000000000000000000000000000");
    const stale = snapshot(); stale.available = false; answer(f.requests[0], stale);
    assert.equal(f.states.at(-1).status, "unavailable"); assert.equal(client.action("light"), false); assert.equal(client.action("hdmi1"), false);
});
test("manual network plan stays fixed, bounded and separate from gateway authorization", () => {
    const f = fixture(), network = api.networkTest(f.options);
    f.advance(100000); assert.equal(f.requests.length, 0); assert.equal(network.start(), true); assert.equal(network.start(), false);
    let sent = 0, received = 0;
    for (let i = 0; i < 9; i++) {
        const r = f.requests[i].request;
        assert.match(r.url, /^https:\/\/speed\.cloudflare\.com\/(?:__down\?bytes=(?:0|100000|1000000|5000000)|__up)$/);
        assert.equal(r.headers.Authorization, undefined); assert.ok(r.timeout <= 10000);
        const bytes = r.method === "POST" ? 0 : Number(r.url.split("=")[1]); sent += r.body ? r.body.length : 0; received += bytes;
        assert.ok(!r.body || r.body.length <= 5000000); f.advance(100); f.requests[i].done(null, { bytes });
    }
    assert.equal(sent + received, 12200000); assert.equal(network.running(), false);
    const result = f.states.at(-1); assert.equal(result.status, "complete"); assert.equal(result.latencyMs, 100); assert.ok(result.downloadMbps > 0); assert.ok(result.uploadMbps > 0); assert.equal(f.timers.size, 0);
});
test("network cancel suppresses late results, deadline aborts, failure is unavailable not zero", () => {
    const f = fixture(), network = api.networkTest(f.options);
    network.start(); network.cancel(); assert.equal(f.requests[0].aborted, true); f.requests[0].done(null, { bytes: 0 }); assert.equal(f.requests.length, 1); assert.equal(f.states.at(-1).status, "cancelled");
    network.start(); f.advance(60000); assert.equal(f.requests[1].aborted, true); assert.equal(f.states.at(-1).status, "timeout");
    network.start(); f.requests[2].done("NETWORK_UNAVAILABLE"); assert.deepEqual(f.states.at(-1), { status: "unavailable" });
});
test("wrong download byte counts fail closed", () => {
    const f = fixture(), network = api.networkTest(f.options); network.start();
    for (let i = 0; i < 3; i++) { f.advance(1); f.requests[i].done(null, { bytes: 0 }); }
    f.requests[3].done(null, { bytes: 10 }); assert.equal(f.states.at(-1).status, "unavailable"); assert.equal(f.requests.length, 4);
});
test("XHR uses actual transferred bytes, caps responses and cancellation is silent", () => {
    const instances = [];
    function XHR() { instances.push(this); this.headers = {}; }
    XHR.prototype.open = function (method, url) { this.method = method; this.url = url; };
    XHR.prototype.setRequestHeader = function (key, value) { this.headers[key] = value; };
    XHR.prototype.send = function (body) { this.body = body; };
    XHR.prototype.abort = function () { this.aborted = true; if (this.onabort) this.onabort(); };
    const send = api.xhrTransport(XHR), calls = [];
    send({ url: "https://speed.cloudflare.com/__down?bytes=5", binary: true, headers: {}, maxResponseBytes: 5 }, (...args) => calls.push(args));
    const xhr = instances[0]; xhr.status = 200; xhr.response = new ArrayBuffer(5); xhr.onload(); assert.equal(calls[0][1].bytes, 5);
    const abort = send({ url: "https://x", headers: {} }, (...args) => calls.push(args)); abort(); assert.equal(calls.length, 1);
    send({ url: "https://x", headers: {}, maxResponseBytes: 5 }, (...args) => calls.push(args)); instances[2].onprogress({ loaded: 6 }); assert.equal(calls.at(-1)[0], "RESPONSE_TOO_LARGE"); assert.equal(instances[2].aborted, true);
    send({ url: "https://nas.local/", method: "HEAD", headers: {}, acceptHttpStatus: true }, (...args) => calls.push(args));
    instances[3].status = 401; instances[3].onload(); assert.equal(calls.at(-1)[0], null); assert.equal(calls.at(-1)[1].status, 401); assert.equal(instances[3].withCredentials, false);
    send({ url: "https://nas.local/", method: "HEAD", headers: {}, acceptHttpStatus: true }, (...args) => calls.push(args));
    instances[4].status = 0; instances[4].onload(); assert.equal(calls.at(-1)[0], "HTTP_ERROR");
});
test("manual service probe accepts only origins and fixed paths, never carries gateway auth", () => {
    for (const [origin, path, lan] of [["https://nas.local/secret", "/", false], ["https://user:key@nas.local", "/", false], ["https://nas.local?key=x", "/", false], ["https://nas.local#key", "/", false], ["http://192.168.1.5", "/", false], ["http://example.com", "/", true], ["https://nas.local", "/secret", true], ["http://192.168.999.1", "/", true], ["https://nas.local:99999", "/", false]]) {
        const f = fixture(), probe = api.buildServiceProbe(f.options);
        assert.equal(probe.start(origin, path, lan), false); assert.equal(f.requests.length, 0); assert.equal(f.states.at(-1).status, "unavailable");
    }
    const f = fixture(), probe = api.buildServiceProbe(f.options);
    f.advance(10000); assert.equal(f.requests.length, 0);
    assert.equal(probe.start("http://192.168.1.5:8080", "/health", true), true);
    assert.equal(probe.start("https://nas.local", "/", false), false);
    const request = f.requests[0].request;
    assert.equal(request.method, "HEAD"); assert.equal(request.url, "http://192.168.1.5:8080/health"); assert.deepEqual(request.headers, {}); assert.equal(request.body, undefined); assert.equal(request.timeout, 5000); assert.equal(request.maxResponseBytes, 4096);
    f.requests[0].done(null, { status: 401 }); assert.deepEqual(f.states.at(-1), { status: "reached", httpStatus: 401 });
    probe.start("https://proxy.local", "/identity", false); f.requests[1].done(null, { status: 404 }); assert.deepEqual(f.states.at(-1), { status: "reached", httpStatus: 404 });
});
test("service probe timeout, CORS/status zero and cancellation stay unknown without late results", () => {
    const f = fixture(), probe = api.buildServiceProbe(f.options);
    probe.start("https://nas.local", "/", false); f.requests[0].done("NETWORK_UNAVAILABLE"); assert.equal(f.states.at(-1).status, "unavailable");
    probe.start("https://nas.local", "/", false); f.requests[1].done(null, { status: 0 }); assert.equal(f.states.at(-1).status, "unavailable");
    probe.start("https://nas.local", "/", false); f.advance(5000); assert.equal(f.requests[2].aborted, true); assert.deepEqual(f.states.at(-1), { status: "unavailable", code: "TIMEOUT" });
    f.requests[2].done(null, { status: 200 }); assert.equal(f.states.at(-1).status, "unavailable");
    probe.start("https://nas.local", "/", false); probe.cancel(); assert.equal(f.requests[3].aborted, true); f.requests[3].done(null, { status: 200 }); assert.equal(f.states.at(-1).status, "cancelled"); assert.equal(f.timers.size, 0);
});

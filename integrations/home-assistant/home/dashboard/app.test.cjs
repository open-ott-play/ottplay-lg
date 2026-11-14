"use strict";
const test = require("node:test"), assert = require("node:assert/strict"), fs = require("node:fs"), vm = require("node:vm");
const api = require("./core.js");
function ui(overrides, realClient = false) {
    const html = fs.readFileSync(__dirname + "/index.html", "utf8"), elements = {}, windowEvents = {}, documentEvents = {}, requests = [], visibility = [], timers = new Map();
    let serial = 0, render;
    const doc = { hidden: false, activeElement: null, getElementById: id => elements[id], addEventListener(name, fn) { documentEvents[name] = fn; } };
    function element(tag, id = "") {
        const e = { tagName: tag.toUpperCase(), id, children: [], attributes: {}, textContent: "", value: "", checked: false, disabled: false, hidden: false, offsetWidth: 100, offsetHeight: 50,
            appendChild(child) { this.children.push(child); return child; }, removeChild(child) { this.children.splice(this.children.indexOf(child), 1); },
            get firstChild() { return this.children[0]; }, setAttribute(key, value) { this.attributes[key] = value; }, getAttribute(key) { return this.attributes[key] || null; },
            focus() { doc.activeElement = this; }, blur() { if (doc.activeElement === this) doc.activeElement = null; }, getBoundingClientRect() { return { left: 0, top: 0, width: 100, height: 50 }; } };
        return e;
    }
    for (const m of html.matchAll(/<(\w+)([^>]*\bid="([^"]+)"[^>]*)>/g)) {
        const e = elements[m[3]] = element(m[1], m[3]);
        const type = /\btype="([^"]+)"/.exec(m[2]);
        e.type = type ? type[1] : m[1].toLowerCase() === "input" ? "text" : "";
    }
    const nav = ["home", "apps", "network", "settings"].map(name => { const e = element("button"); e.setAttribute("data-tab", name); return e; });
    function descendants() { const all = [...Object.values(elements), ...nav]; function walk(e) { for (const child of e.children) { all.push(child); walk(child); } } Object.values(elements).forEach(walk); return all; }
    doc.createElement = tag => element(tag);
    doc.querySelectorAll = selector => selector === ".panel" ? ["home", "apps", "network", "settings"].map(id => elements[id]) : selector === "[data-tab]" ? nav : descendants().filter(e => selector === "[data-item]" ? e.getAttribute("data-item") : e.tagName === "BUTTON" || e.tagName === "INPUT" || e.tagName === "SELECT" || e.tagName === "TEXTAREA");
    const client = { visible(value) { visibility.push(value); }, configure() { return true; }, action(id) { requests.push({ action: id }); return true; } };
    const window = { OTTHome: { ...api, createClient(options) { render = options.onState; if (realClient) return api.createClient({ ...options, clock: { set(fn, ms) { const id = ++serial; timers.set(id, { fn, ms }); return id; }, clear(id) { timers.delete(id); } } }); return client; }, xhrTransport() { return (request, done) => { const r = { request, done, aborted: false }; requests.push(r); return () => { r.aborted = true; }; }; } }, addEventListener(name, fn) { windowEvents[name] = fn; } };
    const navigator = { onLine: true };
    Object.assign(window, overrides || {});
    const context = { window, document: doc, navigator, setTimeout(fn, ms) { const id = ++serial; timers.set(id, { fn, ms }); return id; }, clearTimeout(id) { timers.delete(id); }, console, Date, isFinite };
    vm.runInNewContext(fs.readFileSync(__dirname + "/webos-host.js", "utf8"), context);
    vm.runInNewContext(fs.readFileSync(__dirname + "/app.js", "utf8"), context);
    return { window, doc, navigator, windowEvents, documentEvents, elements, nav, requests, visibility, render, timers };
}
test("page hiding and Back abort the real dashboard network controller without restart", () => {
    const u = ui(); u.nav[2].onclick(); u.elements["network-start"].onclick(); assert.equal(u.requests.length, 1);
    u.doc.hidden = true; u.documentEvents.visibilitychange(); assert.equal(u.requests[0].aborted, true); assert.match(u.elements["network-state"].textContent, /cancelled/);
    u.doc.hidden = false; u.documentEvents.visibilitychange(); assert.equal(u.requests.length, 1);
    u.elements["network-start"].onclick(); assert.equal(u.requests.length, 2);
    u.windowEvents.keydown({ keyCode: 461, preventDefault() {} }); assert.equal(u.requests[1].aborted, true); assert.equal(u.elements.home.hidden, false);
});
test("offline/tab switch stop requests and do not autorun a test", () => {
    const u = ui(); u.nav[2].onclick(); u.elements["network-start"].onclick(); u.navigator.onLine = false; u.windowEvents.offline();
    assert.equal(u.requests[0].aborted, true); u.elements["network-start"].onclick(); assert.equal(u.requests.length, 1);
    u.navigator.onLine = true; u.windowEvents.online(); assert.equal(u.requests.length, 1);
    u.elements["network-start"].onclick(); u.nav[0].onclick(); assert.equal(u.requests[1].aborted, true);
});
test("TCP never substitutes a host measurement; real service handles are cancelled", () => {
    const u = ui(); u.nav[2].onclick(); u.elements["tcp-router"].onclick(); assert.match(u.elements["tcp-state"].textContent, /unavailable/);
    let call, cancelled = false;
    u.window.webOS = { service: { request(uri, options) { call = { uri, options }; return { cancel() { cancelled = true; } }; } } };
    u.elements["tcp-router"].onclick(); assert.equal(call.uri, "luna://com.openottplay.home.network"); assert.deepEqual(JSON.parse(JSON.stringify(call.options.parameters)), { target: "router", port: 443 });
    u.doc.hidden = true; u.documentEvents.visibilitychange(); assert.equal(cancelled, true);
    call.options.onSuccess({ returnValue: true, status: "ok", method: "tcp", target: "router", port: 443, ms: 5 }); assert.match(u.elements["tcp-state"].textContent, /cancelled/);
});
test("allowlisted native launch only follows ready snapshot, Back at Home closes optional host", () => {
    const u = ui(), calls = [];
    u.window.OTTHomeHost = { launch(id, kind, done) { calls.push({ id, kind }); done(true); }, close() { calls.push("close"); } };
    const model = { entities: [], actions: [], apps: [{ id: "hdmi1", label: "Console", kind: "hdmi" }] };
    u.render({ status: "ready", model }); const old = u.elements.launchers.children[0]; old.onclick(); assert.deepEqual(calls[0], { id: "hdmi1", kind: "hdmi" });
    u.render({ status: "unavailable" }); old.onclick(); assert.equal(calls.length, 1);
    u.windowEvents.keydown({ keyCode: 461, preventDefault() {} }); assert.equal(calls[1], "close"); assert.equal(u.visibility.at(-1), false);
});
test("actual dashboard uses PalmServiceBridge for exact TCP URI and cancels on hide", () => {
    const u = ui(), calls = [];
    function Palm() { calls.push(this); this.cancelled = false; }
    Palm.prototype.call = function (uri, body) { this.uri = uri; this.body = body; };
    Palm.prototype.cancel = function () { this.cancelled = true; };
    u.window.PalmServiceBridge = Palm;
    u.nav[2].onclick(); u.elements["tcp-internet"].onclick();
    assert.equal(calls[0].uri, "luna://com.openottplay.home.network/probe"); assert.deepEqual(JSON.parse(calls[0].body), { target: "internet", port: 443 });
    const callback = calls[0].onservicecallback;
    u.doc.hidden = true; u.documentEvents.visibilitychange(); assert.equal(calls[0].cancelled, true);
    callback(JSON.stringify({ returnValue: true, status: "ok", method: "tcp", target: "internet", port: 443, ms: 5 }));
    assert.match(u.elements["tcp-state"].textContent, /cancelled/);
});
test("packaged Palm dashboard installs constant OTT close and does not override injected close", () => {
    const calls = [];
    function Palm() { calls.push(this); }
    Palm.prototype.call = function (uri, body) { this.uri = uri; this.body = body; };
    Palm.prototype.cancel = function () { this.cancelled = true; };
    const u = ui({ PalmServiceBridge: Palm });
    assert.equal(calls.length, 0, "Installing a local bridge performs no Luna call");
    u.windowEvents.keydown({ keyCode: 461, preventDefault() {} });
    assert.equal(calls[0].uri, "luna://com.webos.applicationManager/launch"); assert.deepEqual(JSON.parse(calls[0].body), { id: "com.openottplay.player.native" });
    calls[0].onservicecallback('{"returnValue":false}'); assert.match(u.elements["home-message"].textContent, /Return to OTT is unavailable/);
    const close = () => {};
    const other = ui({ PalmServiceBridge: Palm, OTTHomeHost: { close } }); assert.equal(other.window.OTTHomeHost.close, close);
});
test("manual NAS/proxy HTTP probe is isolated and cancelled on lifecycle and test changes", () => {
    const u = ui(); assert.equal(u.requests.length, 0); u.nav[2].onclick();
    u.elements["service-origin"].value = "https://nas.local"; u.elements["service-path"].value = "/health";
    function start() { u.elements["service-form"].onsubmit({ preventDefault() {} }); }
    start(); assert.equal(u.requests[0].request.method, "HEAD"); assert.deepEqual(Object.keys(u.requests[0].request.headers), []);
    u.requests[0].done(null, { status: 401 }); assert.match(u.elements["service-state"].textContent, /HTTP endpoint reached.*401/); assert.equal(u.elements.download.textContent, "");
    start(); u.doc.hidden = true; u.documentEvents.visibilitychange(); assert.equal(u.requests[1].aborted, true);
    u.doc.hidden = false; u.documentEvents.visibilitychange(); assert.equal(u.requests.length, 2);
    start(); u.windowEvents.keydown({ keyCode: 461, preventDefault() {} }); assert.equal(u.requests[2].aborted, true);
    u.nav[2].onclick(); start(); u.navigator.onLine = false; u.windowEvents.offline(); assert.equal(u.requests[3].aborted, true);
    u.navigator.onLine = true; u.windowEvents.online(); start(); u.elements["network-start"].onclick(); assert.equal(u.requests[4].aborted, true); assert.equal(u.requests[5].request.url.indexOf("https://speed.cloudflare.com/"), 0);
    start(); assert.equal(u.requests[5].aborted, true); u.nav[0].onclick(); assert.equal(u.requests[6].aborted, true);
});

function press(u, keyCode, extra) {
    let prevented = false;
    u.windowEvents.keydown(Object.assign({ keyCode, preventDefault() { prevented = true; } }, extra));
    return prevented;
}
function arrange(u, entries) {
    for (const e of [...Object.values(u.elements), ...u.nav]) e.disabled = true;
    for (const [element, left, top] of entries) {
        element.disabled = false;
        element.getBoundingClientRect = () => ({ left, top, width: 100, height: 40 });
    }
}
test("D-pad leaves checkbox and text field for submit without losing checkbox OK or caret keys", () => {
    const u = ui(); u.nav[3].onclick();
    const checkbox = u.elements["trusted-lan"], key = u.elements.key;
    const submit = u.elements["submit-fixture"] = u.doc.createElement("button");
    arrange(u, [[checkbox, 0, 0], [key, 0, 100], [submit, 0, 200]]);
    checkbox.focus(); assert.equal(checkbox.type, "checkbox");
    assert.equal(press(u, 13), true); assert.equal(checkbox.checked, true);
    assert.equal(press(u, 40), true); assert.equal(u.doc.activeElement, key);
    assert.equal(press(u, 37), false); assert.equal(press(u, 39), false); assert.equal(u.doc.activeElement, key);
    assert.equal(press(u, 8), false, "Backspace still edits text");
    assert.equal(press(u, 40), true); assert.equal(u.doc.activeElement, submit);
    assert.equal(press(u, 38), true); assert.equal(u.doc.activeElement, key);
    assert.equal(press(u, 13), false, "native form submission is not intercepted");
    assert.equal(u.requests.length, 0, "focus movement never starts network work");
});
test("first TV Back blurs text entry and retains Connection; next Back returns Home", () => {
    const u = ui(); u.nav[3].onclick(); const key = u.elements.key; key.focus();
    assert.equal(press(u, 40, { isComposing: true }), false); assert.equal(u.doc.activeElement, key);
    assert.equal(press(u, 461), true); assert.equal(u.doc.activeElement, u.nav[3]);
    assert.equal(u.elements.settings.hidden, false); assert.equal(u.elements.home.hidden, true);
    assert.equal(press(u, 461), true); assert.equal(u.elements.home.hidden, false);
    assert.equal(u.requests.length, 0);
});
test("select keeps native option arrows and uses horizontal arrows to leave the field", () => {
    const u = ui(); u.nav[3].onclick();
    const select = u.elements["select-fixture"] = u.doc.createElement("select");
    const button = u.elements["button-fixture"] = u.doc.createElement("button");
    arrange(u, [[select, 0, 0], [button, 150, 0]]); select.focus();
    assert.equal(press(u, 38), false); assert.equal(press(u, 40), false); assert.equal(u.doc.activeElement, select);
    assert.equal(press(u, 39), true); assert.equal(u.doc.activeElement, button);
    assert.equal(press(u, 37), true); assert.equal(u.doc.activeElement, select);
    assert.equal(press(u, 461), true); assert.equal(u.doc.activeElement, u.nav[3]);
});

test("real client refresh cannot overwrite failed or throwing host-close feedback", () => {
    for (const close of [
        (done) => done(false),
        () => {
            throw new Error("host unavailable");
        },
    ]) {
        const u = ui({ OTTHomeHost: { close } }, true);
        u.elements.base.value = "https://gateway.example";
        u.elements.key.value = "x".repeat(32);
        u.elements["connection-form"].onsubmit({ preventDefault() {} });
        const reply = () =>
            u.requests.at(-1).done(null, {
                text: JSON.stringify({
                    version: 1,
                    entities: [],
                    actions: [],
                    apps: [],
                }),
            });
        reply();
        assert.equal(u.elements.connection.textContent, "Home connected");
        u.windowEvents.keydown({ keyCode: 461, preventDefault() {} });
        const message =
            "Return to OTT is unavailable. Open OTT from the TV launcher.";
        assert.equal(u.elements.connection.textContent, "Connecting…");
        assert.equal(u.elements["home-message"].textContent, message);
        reply();
        assert.equal(u.elements.connection.textContent, "Home connected");
        assert.equal(u.elements["home-message"].textContent, message);
        u.nav[2].onclick();
        u.nav[0].onclick();
        assert.equal(u.elements.connection.textContent, "Connecting…");
        assert.equal(u.elements["home-message"].textContent, message);
        reply();
        assert.equal(u.elements["home-message"].textContent, message);
        u.windowEvents.pagehide();
        assert.equal(u.timers.size, 0);
    }
});

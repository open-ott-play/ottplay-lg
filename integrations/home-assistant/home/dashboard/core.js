/* Original portable OTT Home adapter. ES5; no external runtime. */
(function (root, factory) {
    if (typeof module === "object" && module.exports) module.exports = factory();
    else root.OTTHome = factory();
}(this, function () {
    "use strict";
    var DOWN = "https://speed.cloudflare.com/__down?bytes=", UP = "https://speed.cloudflare.com/__up";
    function clock() { var previous = 0; return { now: function () { var value = typeof performance !== "undefined" && typeof performance.now === "function" ? performance.now() : Date.now(); previous = Math.max(previous, value); return previous; }, set: function (fn, ms) { return setTimeout(fn, ms); }, clear: function (id) { clearTimeout(id); } }; }
    function xhrTransport(XHR) {
        return function (request, done) {
            var xhr = new XHR(), finished = false, key;
            function finish(error, result) { if (finished) return; finished = true; done(error, result); }
            try { xhr.open(request.method || "GET", request.url, true); }
            catch (error) { setTimeout(function () { finish("NETWORK_UNAVAILABLE"); }, 0); return function () { finished = true; }; }
            xhr.timeout = request.timeout || 10000;
            xhr.withCredentials = false;
            if (request.binary) { try { xhr.responseType = "arraybuffer"; } catch (ignored) {} }
            try { for (key in request.headers) if (Object.prototype.hasOwnProperty.call(request.headers, key)) xhr.setRequestHeader(key, request.headers[key]); }
            catch (error) { setTimeout(function () { finish("NETWORK_UNAVAILABLE"); }, 0); return function () { finished = true; }; }
            xhr.onload = function () {
                var bytes = xhr.response && typeof xhr.response.byteLength === "number" ? xhr.response.byteLength : null, body = "";
                if (bytes === null) { try { body = xhr.responseText || ""; bytes = body.length; } catch (ignored) {} }
                if (xhr.status === 0 || (!request.acceptHttpStatus && (xhr.status < 200 || xhr.status >= 300))) finish("HTTP_ERROR");
                else finish(null, { text: body, bytes: bytes, status: xhr.status });
            };
            xhr.onerror = function () { finish("NETWORK_UNAVAILABLE"); };
            xhr.ontimeout = function () { finish("TIMEOUT"); };
            xhr.onabort = function () { finish("CANCELLED"); };
            xhr.onprogress = function (event) { if (request.maxResponseBytes && event.loaded > request.maxResponseBytes) { finish("RESPONSE_TOO_LARGE"); xhr.abort(); } };
            try { xhr.send(request.body || null); } catch (error) { setTimeout(function () { finish("NETWORK_UNAVAILABLE"); }, 0); }
            return function () { finished = true; try { xhr.abort(); } catch (ignored) {} };
        };
    }
    function validSnapshot(value) {
        var lists = ["entities", "actions", "apps"], i, j, row, seen;
        if (!value || value.version !== 1) return false;
        for (i = 0; i < lists.length; i++) {
            if (Object.prototype.toString.call(value[lists[i]]) !== "[object Array]" || value[lists[i]].length > 500) return false;
            seen = Object.create(null);
            for (j = 0; j < value[lists[i]].length; j++) {
                row = value[lists[i]][j];
                if (!row || typeof row.id !== "string" || !row.id || typeof row.label !== "string" || seen[row.id]) return false;
                seen[row.id] = true;
                if (i === 0 && (typeof row.state !== "string" || typeof row.kind !== "string" || (row.unit !== undefined && typeof row.unit !== "string"))) return false;
                if (i === 2 && row.kind !== "app" && row.kind !== "hdmi" && row.kind !== "tv") return false;
            }
        }
        return true;
    }
    function baseUrl(value, trustedLAN) {
        // No userinfo, query, fragment or path; HTTPS or loopback HTTP only.
        var parsed = /^(https?):\/\/([A-Za-z0-9.-]+)(?::([0-9]{1,5}))?\/?$/.exec(value), parts, i;
        if (!parsed || (parsed[3] !== undefined && (Number(parsed[3]) < 1 || Number(parsed[3]) > 65535))) return null;
        if (/^[0-9.]+$/.test(parsed[2])) {
            parts = parsed[2].split("."); if (parts.length !== 4) return null;
            for (i = 0; i < parts.length; i++) if (!/^(0|[1-9][0-9]{0,2})$/.test(parts[i]) || Number(parts[i]) > 255) return null;
        }
        var privateAddress = /^http:\/\/(10(?:\.[0-9]{1,3}){3}|192\.168(?:\.[0-9]{1,3}){2}|172\.(?:1[6-9]|2[0-9]|3[01])(?:\.[0-9]{1,3}){2}|[A-Za-z0-9.-]+\.local)(?::[0-9]{1,5})?\/?$/;
        return /^https:\/\/[A-Za-z0-9.-]+(?::[0-9]{1,5})?\/?$/.test(value) || /^http:\/\/(?:127\.0\.0\.1|localhost)(?::[0-9]{1,5})?\/?$/.test(value) || (trustedLAN === true && privateAddress.test(value)) ? value.replace(/\/$/, "") : null;
    }
    function buildServiceProbe(options) {
        var time = options.clock || clock(), pending = null, timer = null, active = false, generation = 0;
        function stop(state) {
            generation++; active = false;
            if (timer !== null) time.clear(timer); timer = null;
            if (pending) pending(); pending = null;
            if (state) options.onState(state);
        }
        return {
            start: function (origin, path, trustedLAN) {
                var normalized, token, handle, complete = false;
                if (active) return false;
                normalized = baseUrl(origin, trustedLAN);
                if (!normalized || (origin.indexOf("http:") === 0 && trustedLAN !== true) || (path !== "/" && path !== "/health" && path !== "/identity")) {
                    options.onState({ status: "unavailable", code: "INVALID_ORIGIN" }); return false;
                }
                active = true; token = ++generation;
                options.onState({ status: "running" });
                timer = time.set(function () { stop({ status: "unavailable", code: "TIMEOUT" }); }, 5000);
                handle = options.transport({ method: "HEAD", url: normalized + path, headers: {}, timeout: 5000, maxResponseBytes: 4096, acceptHttpStatus: true }, function (error, response) {
                    if (!active || token !== generation) return;
                    complete = true; pending = null;
                    if (error || !response || typeof response.status !== "number" || response.status < 100 || response.status > 599) stop({ status: "unavailable", code: error === "TIMEOUT" ? "TIMEOUT" : "UNKNOWN" });
                    else stop({ status: "reached", httpStatus: response.status });
                });
                if (!complete && token === generation) pending = handle;
                return true;
            },
            cancel: function () { if (active) stop({ status: "cancelled" }); },
            running: function () { return active; }
        };
    }
    function createClient(options) {
        var transport = options.transport, time = options.clock || clock(), visible = false, timer = null, abort = null, generation = 0, failures = 0, config = null, model = null, busyAction = false;
        function emit(state) { options.onState(state); }
        function stop() { generation++; if (timer !== null) time.clear(timer); timer = null; if (abort) abort(); abort = null; busyAction = false; }
        function schedule() { if (visible && config) timer = time.set(refresh, Math.min(120000, 15000 * Math.pow(2, failures))); }
        function refresh() {
            var token;
            timer = null;
            if (!visible || !config || abort || busyAction) return;
            token = ++generation;
            emit({ status: "loading", model: model });
            abort = transport({ url: config.base + "/api/home", headers: { Authorization: "Bearer " + config.key }, timeout: 10000 }, function (error, response) {
                var parsed;
                if (token !== generation || !visible) return;
                abort = null;
                if (!error) { try { parsed = JSON.parse(response.text); } catch (ignored) { error = "INVALID_RESPONSE"; } }
                if (!error && !validSnapshot(parsed)) error = "INVALID_RESPONSE";
                if (!error && parsed.available === false) error = "HOME_UNAVAILABLE";
                if (error) { failures = Math.min(failures + 1, 3); model = null; emit({ status: "unavailable", code: error }); }
                else { failures = 0; model = parsed; emit({ status: "ready", model: model }); }
                schedule();
            });
        }
        return {
            configure: function (base, key, trustedLAN) {
                stop(); model = null; config = null; failures = 0;
                base = baseUrl(base, trustedLAN);
                if (!base || typeof key !== "string" || !/^[A-Za-z0-9_-]{32,256}$/.test(key)) { emit({ status: "unconfigured" }); return false; }
                config = { base: base, key: key }; refresh(); return true;
            },
            visible: function (value) { if (visible === value) return; visible = value; stop(); if (value) { if (config) refresh(); else emit({ status: "unconfigured" }); } },
            action: function (id) {
                var found = false, i, token;
                if (!visible || !model || abort || busyAction) return false;
                for (i = 0; i < model.actions.length; i++) if (model.actions[i].id === id) found = true;
                for (i = 0; i < model.apps.length; i++) if (model.apps[i].id === id) found = true;
                if (!found) return false;
                if (timer !== null) time.clear(timer); timer = null; busyAction = true; token = ++generation;
                emit({ status: "acting", model: model });
                abort = transport({ method: "POST", url: config.base + "/api/action", headers: { Authorization: "Bearer " + config.key, "Content-Type": "application/json" }, body: JSON.stringify({ id: id }), timeout: 10000 }, function (error) {
                    if (token !== generation || !visible) return;
                    abort = null; busyAction = false;
                    if (error) { model = null; failures++; emit({ status: "unavailable", code: error }); schedule(); }
                    else refresh();
                });
                return true;
            },
            cancel: function () { stop(); model = null; emit({ status: "cancelled" }); },
            refresh: function () { if (!abort) { if (timer !== null) time.clear(timer); refresh(); } }
        };
    }
    function networkTest(options) {
        var time = options.clock || clock(), transport = options.transport, running = false, pending = null, deadline = null, generation = 0, started = 0;
        function emit(state) { options.onState(state); }
        function stop(status) { generation++; running = false; if (pending) pending(); pending = null; if (deadline !== null) time.clear(deadline); deadline = null; if (status) emit({ status: status }); }
        return {
            start: function () {
                var token, plan, index = 0, latencies = [], downBytes = 0, upBytes = 0, downMs = 0, upMs = 0;
                if (running) return false;
                running = true; token = ++generation; started = time.now();
                // Planned HTTP body traffic 12.2 MB; maximum individual body 5 MB.
                plan = [{ kind: "latency", bytes: 0 }, { kind: "latency", bytes: 0 }, { kind: "latency", bytes: 0 }];
                [100000, 1000000, 5000000].forEach(function (bytes) { plan.push({ kind: "download", bytes: bytes }); });
                [100000, 1000000, 5000000].forEach(function (bytes) { plan.push({ kind: "upload", bytes: bytes }); });
                deadline = time.set(function () { stop("timeout"); }, 60000);
                emit({ status: "running", phase: "latency", plannedBytes: 12200000 });
                function next() {
                    var step, at, request;
                    if (!running || token !== generation) return;
                    if (index === plan.length) {
                        stop();
                        latencies.sort(function (a, b) { return a - b; });
                        emit({ status: "complete", latencyMs: latencies[1], downloadMbps: downMs > 0 ? downBytes * 8 / downMs / 1000 : null, uploadMbps: upMs > 0 ? upBytes * 8 / upMs / 1000 : null, elapsedMs: time.now() - started }); return;
                    }
                    step = plan[index++]; at = time.now(); emit({ status: "running", phase: step.kind, step: index, steps: plan.length });
                    request = { method: step.kind === "upload" ? "POST" : "GET", url: step.kind === "upload" ? UP : DOWN + step.bytes, headers: {}, binary: true, maxResponseBytes: step.kind === "download" ? step.bytes : 65536, timeout: Math.min(10000, 60000 - (at - started)) };
                    if (request.timeout <= 0) { stop("timeout"); return; }
                    if (step.kind === "upload") { request.headers["Content-Type"] = "application/octet-stream"; request.body = new Array(step.bytes + 1).join("0"); }
                    pending = transport(request, function (error, response) {
                        var elapsed;
                        if (!running || token !== generation) return;
                        pending = null; elapsed = time.now() - at;
                        if (error) { stop(error === "TIMEOUT" ? "timeout" : "unavailable"); return; }
                        if (step.kind === "download" && response.bytes !== step.bytes) { stop("unavailable"); return; }
                        if (step.kind === "latency") latencies.push(elapsed);
                        if (step.kind === "download") { downBytes += response.bytes; downMs += elapsed; }
                        if (step.kind === "upload") { upBytes += step.bytes; upMs += elapsed; }
                        next();
                    });
                }
                next(); return true;
            },
            cancel: function () { stop("cancelled"); },
            running: function () { return running; }
        };
    }
    return { xhrTransport: xhrTransport, createClient: createClient, networkTest: networkTest, buildServiceProbe: buildServiceProbe, validSnapshot: validSnapshot, baseUrl: baseUrl };
}));

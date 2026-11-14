/* Original ES5 platform transport. No CDN, arbitrary launch IDs or credentials. */
(function (root, factory) {
    if (typeof module === "object" && module.exports) module.exports = factory();
    else root.OTTHomeWebOS = factory();
}(typeof window !== "undefined" ? window : this, function () {
    "use strict";
    function create(host, time) {
        time = time || { set: function (fn, ms) { return setTimeout(fn, ms); }, clear: function (id) { clearTimeout(id); } };
        function available() { return !!(host.webOS && host.webOS.service && typeof host.webOS.service.request === "function") || typeof host.PalmServiceBridge === "function"; }
        function request(uri, method, parameters, success, failure) {
            var active = true, nativeHandle = null, palm = null;
            function cancel() {
                var handle = nativeHandle, bridge = palm;
                active = false; nativeHandle = null; palm = null;
                if (bridge) { bridge.onservicecallback = function () {}; try { if (typeof bridge.cancel === "function") bridge.cancel(); } catch (ignored) {} }
                if (handle && typeof handle.cancel === "function") { try { handle.cancel(); } catch (ignored) {} }
            }
            function finish(ok, result) { if (!active) return; cancel(); if (ok) success(result); else failure(); }
            try {
                if (host.webOS && host.webOS.service && typeof host.webOS.service.request === "function") {
                    var handle = host.webOS.service.request(uri, { method: method, parameters: parameters,
                        onSuccess: function (result) { finish(!!result && result.returnValue === true, result); }, onFailure: function () { finish(false); } });
                    if (active) nativeHandle = handle;
                } else if (typeof host.PalmServiceBridge === "function") {
                    palm = new host.PalmServiceBridge();
                    palm.onservicecallback = function (raw) {
                        var parsed;
                        if (!active) return;
                        if (typeof raw !== "string" || raw.length > 4096) { finish(false); return; }
                        try { parsed = JSON.parse(raw); } catch (ignored) { finish(false); return; }
                        finish(!!parsed && Object.prototype.toString.call(parsed) === "[object Object]" && parsed.returnValue === true, parsed);
                    };
                    palm.call(uri + "/" + method, JSON.stringify(parameters));
                } else finish(false);
            } catch (ignored) { finish(false); }
            return { cancel: cancel };
        }
        function probe(target, port, success, failure) {
            if ((target !== "router" && target !== "internet") || (port !== 443 && port !== 80 && port !== 53)) { failure(); return { cancel: function () {} }; }
            return request("luna://com.openottplay.home.network", "probe", { target: target, port: port }, success, failure);
        }
        function installClose() {
            var bridge = host.OTTHomeHost, closing = false;
            if (typeof host.PalmServiceBridge !== "function" || (bridge && typeof bridge.close === "function")) return false;
            if (!bridge) bridge = {};
            try {
                bridge.close = function (done) {
                    var finished = false, pending = null, timer;
                    if (closing) return false;
                    closing = true;
                    function finish(ok) {
                        if (finished) return; finished = true; closing = false; time.clear(timer);
                        if (pending) pending.cancel();
                        if (typeof done === "function") done(ok);
                    }
                    timer = time.set(function () { finish(false); }, 5000);
                    var handle = request("luna://com.webos.applicationManager", "launch", { id: "com.openottplay.player.native" }, function () { finish(true); }, function () { finish(false); });
                    if (!finished) pending = handle;
                    return true;
                };
                host.OTTHomeHost = bridge;
                return host.OTTHomeHost.close === bridge.close;
            } catch (ignored) { return false; }
        }
        return { available: available, probe: probe, installClose: installClose };
    }
    return { create: create };
}));

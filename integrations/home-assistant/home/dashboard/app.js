/* Original portable TV dashboard. No Home Assistant credentials or direct HA API. */
(function () {
    "use strict";
    var api = window.OTTHome, activeTab = "home", latest = null, connected = false, tcpHandle = null, tcpTimer = null, tcpGeneration = 0, rememberedFocus = null, lastProbe = null, closeFeedback = "";
    var platform = window.OTTHomeWebOS.create(window);
    platform.installClose();
    function byId(id) { return document.getElementById(id); }
    function text(id, value) { byId(id).textContent = value; }
    function clear(node) { while (node.firstChild) node.removeChild(node.firstChild); }
    function node(tag, className, content) { var result = document.createElement(tag); if (className) result.className = className; if (content !== undefined) result.textContent = content; return result; }
    function allowedApps(id, kind) { var i; if (!latest) return false; for (i = 0; i < latest.apps.length; i++) if (latest.apps[i].id === id && latest.apps[i].kind === kind) return true; return false; }
    function launch(id, kind) {
        var host = window.OTTHomeHost;
        if (!connected || !allowedApps(id, kind)) return;
        if (!host || typeof host.launch !== "function") { client.action(id); return; }
        text("host-message", "Requesting launch…");
        try { host.launch(id, kind, function (result) { text("host-message", result === true ? "Launch requested." : "The TV host could not launch this source."); }); }
        catch (ignored) { text("host-message", "The TV host could not launch this source."); }
    }
    function render(state) {
        var ready = state.status === "ready", model = state.model, focused = document.activeElement && document.activeElement.getAttribute("data-item"), elements = byId("entities"), actions = byId("actions"), apps = byId("launchers"), i, row, card, button, candidates;
        connected = ready; latest = ready ? model : null;
        if (focused) rememberedFocus = focused;
        text("connection", ready ? "Home connected" : state.status === "loading" ? "Connecting…" : state.status === "acting" ? "Sending action…" : state.status === "unconfigured" ? "Connection required" : state.status === "cancelled" ? "Disconnected" : "Home unavailable");
        text("home-message", closeFeedback || (ready ? (model.entities.length ? "Status from your configured home devices." : "No status devices configured on your gateway.") : state.status === "loading" ? "Loading home status…" : state.status === "acting" ? "Waiting for the gateway…" : "Home status unavailable. Check the Connection tab."));
        clear(elements); clear(actions); clear(apps);
        if (!ready) return;
        for (i = 0; i < model.entities.length; i++) {
            row = model.entities[i]; card = node("div", "card"); card.appendChild(node("span", "label", row.label));
            card.appendChild(node("span", "value", (row.state === "unknown" || row.state === "unavailable" ? "Unavailable" : row.state) + (row.unit && row.state !== "unknown" && row.state !== "unavailable" ? " " + row.unit : ""))); elements.appendChild(card);
        }
        for (i = 0; i < model.actions.length; i++) {
            row = model.actions[i]; button = node("button", "card", row.label); button.setAttribute("data-item", "action:" + row.id);
            (function (id) { button.onclick = function () { client.action(id); }; }(row.id)); actions.appendChild(button);
        }
        for (i = 0; i < model.apps.length; i++) {
            row = model.apps[i]; button = node("button", "card", row.label + (row.kind === "hdmi" ? " · HDMI" : "")); button.setAttribute("data-item", "app:" + row.id);
            (function (id, kind) { button.onclick = function () { launch(id, kind); }; }(row.id, row.kind)); apps.appendChild(button);
        }
        if (rememberedFocus) { candidates = document.querySelectorAll("[data-item]"); for (i = 0; i < candidates.length; i++) if (candidates[i].getAttribute("data-item") === rememberedFocus) candidates[i].focus(); }
    }
    var transport = api.xhrTransport(window.XMLHttpRequest);
    var client = api.createClient({ transport: transport, onState: render });
    var network = api.networkTest({ transport: transport, onState: function (state) {
        var running = state.status === "running";
        byId("network-start").disabled = running; byId("network-cancel").disabled = !running;
        if (running) {
            text("network-state", "Testing " + state.phase + (state.step ? " (" + state.step + "/" + state.steps + ")" : "") + "…");
            text("latency", "Unavailable"); text("download", "Unavailable"); text("upload", "Unavailable");
        } else if (state.status === "complete") {
            text("network-state", "Check complete. Measured in this browser.");
            text("latency", state.latencyMs === null ? "Unavailable" : state.latencyMs.toFixed(0) + " ms");
            text("download", state.downloadMbps === null ? "Unavailable" : state.downloadMbps.toFixed(1) + " Mbps");
            text("upload", state.uploadMbps === null ? "Unavailable" : state.uploadMbps.toFixed(1) + " Mbps");
        } else text("network-state", state.status === "cancelled" ? "Check cancelled." : state.status === "timeout" ? "Check timed out. Results unavailable." : "Check unavailable. The browser, endpoint or network could not complete the request.");
    } });
    // Separate unauthenticated transport: never reuses gateway client configuration.
    var serviceProbe = api.buildServiceProbe({ transport: api.xhrTransport(window.XMLHttpRequest), onState: function (state) {
        byId("service-start").disabled = state.status === "running"; byId("service-cancel").disabled = state.status !== "running";
        text("service-state", state.status === "running" ? "Checking HTTP endpoint…" : state.status === "reached" ? "HTTP endpoint reached · status " + state.httpStatus + ". Server health is not verified." : state.status === "cancelled" ? "HTTP check cancelled; reachability unknown." : "HTTP check unavailable; reachability unknown. CORS, TLS or browser policy may block the result.");
    } });
    function cancelTCP() {
        tcpGeneration++; if (tcpTimer !== null) clearTimeout(tcpTimer); tcpTimer = null;
        if (tcpHandle && typeof tcpHandle.cancel === "function") { try { tcpHandle.cancel(); } catch (ignored) {} }
        tcpHandle = null;
    }
    function cancelWork() { if (network.running()) network.cancel(); serviceProbe.cancel(); if (tcpHandle || tcpTimer !== null) text("tcp-state", "TCP probe cancelled; status unknown."); cancelTCP(); }
    function visible() { return !document.hidden && navigator.onLine !== false; }
    function closeUnavailable() { closeFeedback = "Return to OTT is unavailable. Open OTT from the TV launcher."; text("home-message", closeFeedback); syncVisibility(); }
    function syncVisibility() { if (!visible()) cancelWork(); client.visible(visible() && (activeTab === "home" || activeTab === "apps")); }
    function tab(name) {
        var panels = document.querySelectorAll(".panel"), buttons = document.querySelectorAll("[data-tab]"), i;
        cancelWork(); activeTab = name;
        for (i = 0; i < panels.length; i++) panels[i].hidden = panels[i].id !== name;
        for (i = 0; i < buttons.length; i++) buttons[i].setAttribute("aria-selected", buttons[i].getAttribute("data-tab") === name ? "true" : "false");
        syncVisibility();
    }
    function probe(target) {
        var token;
        cancelTCP(); if (network.running()) network.cancel(); serviceProbe.cancel();
        if (!visible() || !platform.available()) { text("tcp-state", "TCP probe unavailable: install the optional TV network service."); return; }
        if (lastProbe !== null && Date.now() - lastProbe < 15000) { text("tcp-state", "Wait 15 seconds between TCP probes."); return; }
        lastProbe = Date.now();
        token = tcpGeneration; text("tcp-state", "Checking " + target + " TCP port 443…");
        tcpTimer = setTimeout(function () { if (token === tcpGeneration) { cancelTCP(); text("tcp-state", "TCP probe timed out; status unknown."); } }, 10000);
        function finish(message) { if (token !== tcpGeneration) return; cancelTCP(); text("tcp-state", message); }
        try {
            var handle = platform.probe(target, 443, function (result) {
                    if (result && result.returnValue === true && result.status === "ok" && result.method === "tcp" && result.target === target && result.port === 443 && typeof result.ms === "number" && isFinite(result.ms) && result.ms >= 0) finish(target + " TCP connect: " + result.ms.toFixed(0) + " ms (port 443).");
                    else finish(target + " TCP status unknown. A closed port does not establish network failure.");
                }, function () { finish(target + " TCP probe unavailable; status unknown."); });
            if (token === tcpGeneration) tcpHandle = handle;
        } catch (ignored) { finish("TCP probe unavailable; status unknown."); }
    }
    var nav = document.querySelectorAll("[data-tab]"), i;
    for (i = 0; i < nav.length; i++) nav[i].onclick = function () { tab(this.getAttribute("data-tab")); };
    byId("network-start").onclick = function () { if (visible()) { cancelTCP(); serviceProbe.cancel(); network.start(); } };
    byId("network-cancel").onclick = function () { network.cancel(); cancelTCP(); };
    byId("tcp-router").onclick = function () { probe("router"); };
    byId("tcp-internet").onclick = function () { probe("internet"); };
    byId("service-form").onsubmit = function (event) {
        event.preventDefault();
        if (!visible()) return;
        if (network.running()) network.cancel(); cancelTCP();
        serviceProbe.start(byId("service-origin").value.replace(/^\s+|\s+$/g, ""), byId("service-path").value, byId("service-trusted-lan").checked);
    };
    byId("service-cancel").onclick = function () { serviceProbe.cancel(); };
    byId("connection-form").onsubmit = function (event) {
        event.preventDefault();
        var ok = client.configure(byId("base").value.replace(/^\s+|\s+$/g, ""), byId("key").value, byId("trusted-lan").checked);
        text("settings-state", ok ? "Connection set for this session." : "Enter an HTTPS gateway origin and client key. For trusted private HTTP, enable the LAN option.");
        byId("key").value = "";
        if (ok) { tab("home"); nav[0].focus(); }
    };
    byId("disconnect").onclick = function () { cancelWork(); client.configure("", ""); byId("key").value = ""; latest = null; connected = false; text("settings-state", "Disconnected. Session key cleared."); };
    document.addEventListener("visibilitychange", syncVisibility);
    window.addEventListener("offline", syncVisibility); window.addEventListener("online", syncVisibility);
    window.addEventListener("pagehide", function () { cancelWork(); client.visible(false); });
    window.addEventListener("keydown", function (event) {
        var code = event.keyCode, focus = document.activeElement, tag = focus && focus.tagName,
            select = tag === "SELECT", editing = tag === "TEXTAREA" || (tag === "INPUT" && !/^(checkbox|radio|button|submit|reset|image|hidden)$/.test(focus.type)),
            all, candidates = [], current, best = null, score = Infinity, j, rect, dx, dy, distance;
        // Keep composition/IME events native. Text caret and select option keys
        // are handled below independently of remote spatial navigation.
        if (event.isComposing || code === 229) return;
        if (code === 461 || code === 27 || code === 10009 || (code === 8 && !editing)) {
            event.preventDefault(); cancelWork();
            if (editing || select) {
                if (typeof focus.blur === "function") focus.blur();
                for (j = 0; j < nav.length; j++) if (nav[j].getAttribute("data-tab") === activeTab) { nav[j].focus(); break; }
                return;
            }
            if (activeTab === "home" && window.OTTHomeHost && typeof window.OTTHomeHost.close === "function") {
                client.visible(false);
                try { window.OTTHomeHost.close(function (ok) { if (ok === false) { closeUnavailable(); } }); } catch (ignored) { closeUnavailable(); }
                return;
            }
            tab("home"); nav[0].focus(); return;
        }
        if (code === 13 && focus && focus.type === "checkbox") { event.preventDefault(); focus.checked = !focus.checked; return; }
        if (code < 37 || code > 40 || (editing && (code === 37 || code === 39)) || (select && (code === 38 || code === 40))) return;
        event.preventDefault(); all = document.querySelectorAll("button,input,select,textarea");
        for (j = 0; j < all.length; j++) if (!all[j].disabled && all[j].offsetWidth && all[j].offsetHeight) candidates.push(all[j]);
        if (!focus || !focus.getBoundingClientRect || candidates.indexOf(focus) < 0) { if (candidates.length) candidates[0].focus(); return; }
        current = focus.getBoundingClientRect();
        for (j = 0; j < candidates.length; j++) {
            if (candidates[j] === focus) continue; rect = candidates[j].getBoundingClientRect(); dx = rect.left + rect.width / 2 - (current.left + current.width / 2); dy = rect.top + rect.height / 2 - (current.top + current.height / 2);
            if ((code === 37 && dx >= -2) || (code === 39 && dx <= 2) || (code === 38 && dy >= -2) || (code === 40 && dy <= 2)) continue;
            distance = (code === 37 || code === 39) ? Math.abs(dx) + 3 * Math.abs(dy) : Math.abs(dy) + 3 * Math.abs(dx);
            if (distance < score) { score = distance; best = candidates[j]; }
        }
        if (best) best.focus();
    });
    syncVisibility(); nav[0].focus();
}());

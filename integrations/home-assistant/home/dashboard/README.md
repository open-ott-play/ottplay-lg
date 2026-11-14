# Portable OTT Home dashboard

Original standalone frontend: `index.html`, `style.css`, `core.js`, `webos-host.js`, `app.js`. No package installation, CDN, framework, device connection or build step. Copy these five files into a static document root or approved host package. Shared player settings adapter and optional LG companion package use these same runtime assets. Device-specific acceptance is separate.

Run pure tests with:

```sh
node --test core.test.cjs app.test.cjs webos-host.test.cjs
```

## Gateway contract

The Connection screen accepts an HTTPS origin and a **gateway client access key**, never a Home Assistant token. Keys must match the bridge’s 32–256 character `[A-Za-z0-9_-]` alphabet; Home Assistant JWTs are rejected. The key stays only in JS memory, is cleared from the input after submission, and is never stored in URLs, storage, exports, logs or Cloudflare requests. Disconnect replaces the in-memory configuration. Refreshing the page requires reconnecting.

Loopback HTTP is available for development. A separate, explicit trusted-LAN checkbox allows private IPv4 or `.local` HTTP origins; it states that HTTP sends the gateway key without encryption. Prefer HTTPS. The address must be an origin without credentials, path, query or fragment. The gateway must allow the actual UI origin through its configured CORS allowlist; the UI does not bypass browser mixed-content/private-network restrictions. Serve both over HTTPS for TV/LAN use when required by the browser.

Requests:

- `GET /api/home`, `Authorization: Bearer <client key>`; snapshot `{version:1,available?:boolean,entities:[{id,label,state,unit?,kind}],actions:[{id,label}],apps:[{id,label,kind:'app'|'hdmi'|'tv'}]}`. Strings render using `textContent`, never HTML. `available:false`, malformed replies, HTTP failures and transport failures clear actionable state.
- `POST /api/action`, same authorization, `Content-Type: application/json`, body exactly `{id}`. IDs must be in the latest ready snapshot’s actions or apps. No browser-selected HA entity, service, URL or source is sent. The bridge must revalidate authorization, source availability and its own allowlist. POSTs are never automatically retried; an uncertain timeout stays uncertain until a later status refresh.

Status requests are single-flight, every 15 seconds after success, with failure delays of 30/60/120 seconds. They run only on Home or Apps while the document is visible and browser online. Network/Connection tabs suspend status requests. No status request starts until a user configures the gateway. Hidden, offline and pagehide events cancel in-flight work; stale callbacks cannot restore actionable state.

## Optional installed host bridge

If present, `window.OTTHomeHost.launch(id, kind, callback)` receives only an app ID/kind from the latest ready snapshot; callback `true` means launch requested. It must independently allowlist these IDs. Without a host bridge the same card posts its ID to `/api/action`, letting the gateway select an exact configured TV app/HDMI source through Home Assistant. No arbitrary launch URL is accepted.

`window.OTTHomeHost.close(callback)` is optional. The bundled local `webos-host.js` installs this method only when a real `PalmServiceBridge` constructor exists and no close method is already injected. It requests only the constant app `com.openottplay.player.native` through applicationManager, uses a five-second deadline and single-flight guard, cancels/ignores late replies, and never retries automatically. Failed return requests stay closed and display an unavailable message. It does not install a native arbitrary-app launch method; cards continue using the BFF fallback. Back on Home cancels work and calls close to return to OTT. While a text field or select is focused, the first TV Back leaves entry and focuses the current tab without changing it; a later Back elsewhere cancels work and returns Home. Backspace remains native text editing. A browser without this bridge remains on Home. No `postMessage` listener or credential messages are installed. An embedding parent must inject a trusted bridge directly or implement a separate origin-checked adapter.

## Manual diagnostics

Cloudflare test endpoints are fixed HTTPS `https://speed.cloudflare.com/__down?bytes=N` and `https://speed.cloudflare.com/__up`. The synthetic plan makes three zero-byte HTTP round trips, then downloads and uploads 100 KB, 1 MB and 5 MB each. Planned body traffic totals **12.2 MB**, plus protocol overhead and small upload replies; each planned request body is at most **5 MB**. Responses have progress-based caps (download body size or 64 KB for other responses); the client cannot prevent bytes already in OS/browser buffers at cancellation. A 60-second overall deadline and 10-second per-request timeout apply. No diagnostics autorun or restart after visibility returns.

Latency is median HTTP round-trip duration, not ICMP ping. Download/upload rates use completed requested/transferred bytes divided by full HTTP request time. The UI labels **Source HTTP download speed** separately from media bitrate. Unavailable, cancelled and timed-out tests produce no numeric substitute. Results are from this browser only; no gateway/desktop measurement stands in for the TV. Slow links that cannot finish the bounded plan return unavailable rather than a fabricated estimate. Cloudflare CORS, trust-store and endpoint availability still need real target-browser acceptance.

The separate **NAS / proxy HTTP reachability** form makes one manual `HEAD` request to an entered origin plus one selected fixed path (`/`, `/health`, `/identity`). Userinfo, custom paths, query strings, fragments, invalid IPv4 and invalid ports are rejected. HTTPS is the default; private-LAN HTTP requires its own explicit checkbox. This uses a separate injected transport, a five-second outer/XHR deadline, no request body and a 4 KB response progress cap. Any observable HTTP response status (including 401, 404 or 500) means **HTTP endpoint reached**, never server healthy. CORS, TLS, transport failures, status zero and timeout mean unavailable/unknown, never proof the NAS is down. Browser CORS/mixed-content restrictions are not bypassed. The check does not inspect media or decoder health. Starting it stops speed/TCP work; starting speed/TCP work or hiding/leaving the page cancels it. There is no autorun.

The HTTP endpoint transport passes no BFF Authorization header, client key or body. `withCredentials=false` excludes cross-origin cookies. Same-origin ambient cookies are controlled by the browser and cannot be forcibly omitted through legacy XMLHttpRequest; the provided BFF sets no cookies. This is a browser limitation, not a credential-export mechanism. No gateway authorization configuration is shared with this transport.

Optional TCP buttons use actual `webOS.service.request('luna://com.openottplay.home.network', {method:'probe',parameters:{target:'router'|'internet',port:443},…})` when present. Without that library, the bundled ES5 adapter constructs a real `PalmServiceBridge`, calls the exact `luna://com.openottplay.home.network/probe` URI with JSON `{target,port}`, retains the bridge until response/cancellation, and parses at most 4,096 characters of JSON. Invalid JSON, oversized replies and non-true `returnValue` fail closed. Native cancellation and generation guards suppress late callbacks. This requires no CDN or webOSTV.js. No desktop/gateway fallback probe is run. Success requires `{returnValue:true,status:'ok',method:'tcp',target,port:443,ms:<finite nonnegative number>}` with matching target. Other responses show unknown. Requests are manual, at least 15 seconds apart, with a 10-second UI deadline, cancel-handle use when available, and generation guards against late callbacks. The optional service independently owns enablement, endpoints, budgets and cancellation; dashboard cancellation alone cannot terminate a service socket if its returned handle lacks `cancel()`. A closed router port does not prove general network failure.

## Verification and limits

Tests inject clocks/transports and a small DOM harness. They cover transfer budgets, byte-count mismatch, response caps, timeout/cancel/late callbacks, hidden/offline/tab/Back lifecycle, no autorun, allowlisted action/app dispatch, stale/disconnected launch refusal, unavailable snapshots, key grammar, auth isolation, polling/backoff and host close. Tests perform no network requests and write no Home Assistant state.

Runtime sources use ES5 JavaScript and XMLHttpRequest. Layout uses plain CSS with responsive cards, visible focus, large controls, text status, native touch/click, geometric D-pad navigation and OK/Back handling. Native keyboard/TV focus behavior, old webOS CSS/CA/TLS/CORS support, real remote key mapping, host injection and Home Assistant pairing require target acceptance. No successful TV, network or HA measurement is claimed by these synthetic tests.

## Remote focus and source-package boundary

Checkboxes use arrow-key spatial navigation and OK toggles their checked state. Text fields keep Left/Right for the native caret and use Up/Down to reach neighboring controls, including the form submit button. Select fields keep native Up/Down option selection and use Left/Right to leave the field. Composition/IME events remain native; the first TV Back blurs text/select entry before leaving its current tab. These rules are exercised through the actual app keyboard handler; a real TV keyboard still needs device acceptance.

The small `package.json` marks this source directory CommonJS so `.cjs` tests can load the same ES5 `.js` sources even inside a parent repository with `type: module`. It is source/test metadata, not a sixth browser runtime asset. Public stagers continue copying only the five fixed HTML/CSS/JS files; tests, package metadata and private bridge configuration are not runtime exports.

# Optional TCP diagnostic service

Package this service only with the optional app `com.openottplay.home`.
Its exact Luna API is `luna://com.openottplay.home.network/probe` with
`{"target":"router"|"internet","port":443|80|53}`. It has no dependency on
Home interception, root, HA, shell commands or ICMP. Default `enabled:false`;
explicitly configure this component to enable requests. There is no background
poller. Installation alone sends no probe.

The router endpoint is the current connected IPv4 gateway returned by the
read-only connectionmanager query, restricted to private IPv4. The internet
endpoint is fixed in network.json to either 1.1.1.1 or 8.8.8.8; callers cannot
supply addresses/hostnames/URLs. One request runs at a time, minimum 15 seconds
between starts, gateway query <=1.5 seconds and TCP connect <=2.5 seconds.

Success response: `returnValue:true,target,port,method:"tcp",status,ms`.
Status is `ok` or `unavailable`; only ok has numeric ms. Closed TCP reason codes are TIMEOUT, CONNECT_FAILED and ENDPOINT_UNAVAILABLE. This is TCP
connection latency and is not ICMP ping, throughput, DNS success or proof the
whole internet is unavailable when a particular port refuses connection.
Policy errors return `returnValue:false,status:"unavailable",code,ms:null` (DISABLED, INVALID_ARGUMENT,
BUSY, RATE_LIMITED). No target URLs or credentials are returned or logged.

`node test.cjs` exercises pure validation and injected fake sockets, with no
network. Physical webOS permissions and connection behavior remain unqualified.
Preserve LICENSE and PROVENANCE.md with any packaged service.

The service requests only the documented `network.query` ACG for connectionmanager status. Older firmware permissions still require physical validation; no elevated/private API fallback is implemented.

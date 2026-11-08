# Security policy

Security issues in OTTPlay LG belong to the maintainers of this repository.
Do not send reports about this fork to the upstream project's support address.

Use GitHub private vulnerability reporting when the repository offers it.
Otherwise open an issue requesting a private contact, without vulnerability
instructions, credentials or private logs. No response-time guarantee is implied.

Release builds must exclude developer trigger surfaces and must pass the native
ELF/package checks. Build receipts must identify the exact source, configuration
and linked dependencies. Never include provider secrets, device configuration or
firmware libraries in a public source/binary distribution.

The runtime separates protected settings, remote-command authentication and
platform effects from presentation. Changes to those boundaries require tests
for rejected, stale and uncertain operations as well as successful operations.

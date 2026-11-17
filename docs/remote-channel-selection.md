# Remote channel numbers and temporary media

A one-shot Plex or Local command plays a temporary media queue without changing
the installation's configured provider. A subsequent numeric play command uses
the configured channel catalogue, not the position in that temporary queue:

```sh
ott <player> plex shuffle "Movies"
ott <player> 1
```

The second command selects configured channel 1. It does not require selecting
the M3U provider separately when M3U is already the configured source. It also
does not force the first M3U profile when another source is configured.

Numbers are canonical catalogue numbers across categories. The saved provider,
profile and category remain unchanged. Normal parental and kiosk checks still
apply. Physical remote digits, queue next/previous and receipt-based catalogue
commands keep their existing scope.

The native receiver retains one complete configured catalogue at the first
successful temporary handoff. Later temporary queues keep that same base.
Retention requires a durable, enabled source and bounded metadata: an 8 MiB
encoded catalogue cap and a separate capacity-based admission estimate. This is
not a total-process memory limit. Incomplete, managed or unsupported base views,
including an initial Favorites view, do not grant fallback authority.

If a borrowed server-managed stream owns an active playback lease, stop that
stream or select the configured provider explicitly before numeric handoff. This
prevents preparation from closing its server session before the new source is
admitted.

If no valid base was retained, or its settings/source changed, numeric play is
rejected instead of selecting a temporary queue item. Return to the configured
provider and finish loading its catalogue, then start a new temporary queue.
Updating an application does not manufacture a retained catalogue for an older
in-memory queue.

The receiver validates the request before replacing the queue and checks the
matching native admission receipt before publishing the configured catalogue.
A successful dispatch is not proof of first-frame output: later network or
decoder failures remain playback failures and cannot promise rollback to the
previous decoder.

# Source and licences

This optional adapter derives from MBS Integrations LG-TV-HA-Home-Launcher,
commit `204d4ba713645c7714bf6bfdc11552958972c739`:
https://github.com/mbsintegrations/LG-TV-HA-Home-Launcher/tree/204d4ba713645c7714bf6bfdc11552958972c739/helper

Copyright (C) 2026 MBS Integrations contributors. Adaptations and new adapter
code are distributed under GPL-3.0-only; see LICENSE. The native input helper
also derives from clean-your-tv-lg-webos commit
`9558611134270f3744658f0f6619ad0e67c5e37b`, Copyright (c) 2026 Gathin and
OmkarK97, MIT. Preserve the complete UPSTREAM-LICENSE, including its original
font notice; no upstream font or artwork is bundled here.

UPSTREAM-FILES.json records the original bytes of copied source. Changes include
package paths, bounded bootstrap logging, credential-free OTT-only control,
explicit probe/trial/accepted enablement, and local-only packaging safeguards.
No Home Assistant credentials, watchers, global input enablement, root exploit,
system partition change or boot hook is included. The TV's own Node runtime and
webos-service module are used; Node, Zig and LG tooling are not redistributed.

The optional app icon is the existing OTTPlay pkg/icon.png from this repository, reused under its existing project licence.

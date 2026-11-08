# Classic and Modern native migration

The target is the complete shared OTTPlay experience rendered with SDL. Both
presentations use one source registry, ordered catalogue, installation-level
Favorites collection and playback coordinator.

## Starting point

The imported source implements both presentations, major provider families,
remote control, local discovery and native media handling. This baseline does
not establish full shared-player parity. SOURCE-PROVENANCE.json identifies the
exact import; development now changes ordinary source directly.

## First implemented boundary

Finite remote `seek` and `seek_relative` requests now call the pinned canonical
core through a native QuickJS adapter. Native code retains command admission,
source identity, decoder effects and persistence. The adapter exposes no network
or filesystem callbacks. The same 35 decision cases run in Node and QuickJS;
application tests also exercise the real command path.

CI separates source contracts, complete host tests and an ARM shared-core build.
The ARM check compiles against the webOS SDK, checks generated C bindings and
links a probe against the SDK's libc without executing it. This prevents a host
success from concealing an incompatible television ABI. It is not a complete
application package or physical-device acceptance.

## Delivery order

1. Establish independent source checks, host compilation, ARM build and package
   receipts, retaining application identity and licensing.
2. Connect the canonical core to real native call sites, starting with finite
   media seek decisions compared across Node and QuickJS. Continue with source
   identity, catalogues, sessions, history and Favorites.
3. Align Classic list/preview/details and Modern Home/Library/live lists. Use one
   measured geometry model for drawing, focus and hit testing. Reuse licensed
   fonts, translations, artwork and semantic design tokens.
4. Complete provider/category/profile parity, mixed Favorites transfer, resume,
   progressive local loading, archive controls, audio/subtitle options, remote
   settings, kiosk and standby.
5. Qualify both modes on physical LG hardware, then publish the IPK, matching
   source, metadata and checksums for Developer Mode and Homebrew.

## Behavior contracts

- Mode switching redraws state without reopening media or reloading providers.
- Focus, selected item and playing item are distinct stable identities.
- Passive information preserves navigation and volume input; explicitly active
  controls own their input until closed.
- Normalize remote/keyboard/pointer/touch before dispatch. Desktop plain arrows
  change volume; modified seek has explicit priority.
- Stale loads and actions cannot affect a new source, including reused item IDs.
- Favorites mix providers/profiles and bind to receiver-local sources. Transfers
  do not carry credentials or grant access to files and services.
- A save is successful only after confirmed persistence. Uncertain writes do not
  show a success toast or overwrite unrelated settings.
- Kiosk, parental admission, standby and platform lifecycle precede UI actions.

## Qualification gaps

Protected Save on LG, complete settings import/export, legacy local Favorite
bindings, some HLS formats and complete visual parity require further work or
physical acceptance. An implementation, fixture or version number does not close
these items. The optional companion needs its own source/build qualification.

Record source checks, host behavior, simulator graphics, ARM package verification
and physical-TV observations separately for each milestone.

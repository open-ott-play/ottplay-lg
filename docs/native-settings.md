# Native LG settings

Use `ott PLAYER settings schema` to discover settings supported by the running build. Read a value with `ott PLAYER setting KEY` and save it with `ott PLAYER setting KEY VALUE`. There are 45 schema entries, including two read-only entries. Menu visibility does not add a transport capability that the current source lacks.

Provider presets configure connections and profiles. Player application settings are applied separately through the discovered schema. Retaining an opaque key in a saved state document does not establish a native runtime implementation.

## PIN policy

A missing PIN record uses the shared default PIN, `1234`, and channel protection is enabled by default. An explicitly disabled PIN and a custom native PIN retain their meaning. Use the player's PIN editor to unlock or change it through the protected secret-verification workflow.

Membership comes from the source category and preserved source protection metadata. Admission checks the current source and selected item. A title, list position or renamed browsing category cannot grant access. Background results must not override newer protection decisions. Remote playback receives a PIN-required refusal while access is locked.

Provider-specific manual protection lists (`parentalArray`) are not transferred by native provider presets. Their import is a separate compatibility gap.

## Supported schema

`interfaceMode` defaults to Classic (`0`) on a fresh install, matching the shared player default. A saved choice of Classic (`0`) or Modern (`1`) takes precedence and persists across restarts. Font scale is a percentage, default 100. `osdOpacity` is a level from 0 to 10. `players` is read-only because this build owns a native decoder pipeline. Subtitle appearance uses the native token vocabulary exposed in the schema.

The [schema implementation](../rust-modules/src/app/ott_settings.rs) validates these settings before persistence; application and player code apply them in the declared scope.

The `showNumber`, `showName`, `showProgram`, `showDescription` and `showProgress` flags belong to the provider and are shared by its profile slots. They control the Classic channel list. `pageSize`, `listPosition` and `arrowFun` also configure Classic lists; `seek13Duration` controls eligible numeric-key seeking. These settings do not redefine fullscreen arrow keys.

`nativeSubtitleSize`, `nativeSubtitlePosition` and `nativeSubtitleTone` configure external-source subtitles; Plex keeps its existing profile subtitle preferences. Native text appearance applies to text subtitles; image subtitles keep their artwork. `m3uMediaUrl` is a private, write-only M3U media attachment, applied on the next matching Favorite selection rather than immediately restarting playback.

| Setting | Type | Default | Access |
| --- | --- | --- | --- |
| `uiFontScale` | integer | `100` | read/write |
| `players` | integer | `0` | read-only |
| `interfaceMode` | integer | `0` | read/write |
| `nativeSubtitleFont` | string | `"inter_bold"` | read/write |
| `nativeSubtitleColor` | string | `"white"` | read/write |
| `nativeSubtitleOutline` | string | `"normal"` | read/write |
| `nativeSubtitleBackground` | string | `"transparent"` | read/write |
| `menuShowProviders` | integer | `1` | read/write |
| `menuShowProfiles` | integer | `1` | read/write |
| `menuShowCatalog` | integer | `1` | read/write |
| `menuShowCategories` | integer | `1` | read/write |
| `menuShowFavorites` | integer | `1` | read/write |
| `menuShowSearch` | integer | `1` | read/write |
| `menuShowDescription` | integer | `1` | read/write |
| `menuShowPlay` | integer | `1` | read/write |
| `menuShowFavorite` | integer | `1` | read/write |
| `menuShowRefresh` | integer | `1` | read/write |
| `menuShowAudio` | integer | `1` | read/write |
| `menuShowSubtitles` | integer | `1` | read/write |
| `menuShowQuality` | integer | `1` | read/write |
| `menuShowSpeed` | integer | `1` | read/write |
| `menuShowStats` | integer | `1` | read/write |
| `m3uMediaUrl` | string | `unset` | write-only |
| `nativeSubtitleSize` | string | `"medium"` | read/write |
| `nativeSubtitlePosition` | string | `"low"` | read/write |
| `nativeSubtitleTone` | string | `"white"` | read/write |
| `volumeStep` | integer | `5` | read/write |
| `sleepTimeout` | integer | `0` | read/write |
| `infoTimeout` | integer | `5` | read/write |
| `osdOpacity` | integer | `7` | read/write |
| `permanentTime` | integer | `0` | read/write |
| `pageSize` | integer | `25` | read/write |
| `listPosition` | integer | `0` | read/write |
| `seek13Duration` | integer | `15` | read/write |
| `arrowFun` | integer | `0` | read/write |
| `showNumber` | integer | `1` | read/write |
| `showName` | integer | `1` | read/write |
| `showProgram` | integer | `1` | read/write |
| `showDescription` | integer | `1` | read/write |
| `showProgress` | integer | `1` | read/write |
| `uiColorConfig` | string | `""` | read/write |
| `nativeSubtitleMode` | string | `"off"` | read/write |
| `nativeSubtitleLanguage` | string | `""` | read/write |
| `nativeEpgApiV1` | string | `""` | read/write |
| `nativeInputProbeV1` | string | `""` | read-only |

## Other runtime configuration

Provider credentials, profiles, notifications, categories, Favorites, kiosk designation, PIN operations and playback checkpoints have dedicated contracts.

The shared keys `bufSize`, `liveRecoveryAttempts`, `liveRecoveryTimeout`, `streamMaxHeight`, `streamMaxBitrate`, `preferredAudioLanguage`, `preferredSubtitleLanguage`, `resumeWithTenSecondRewind` and `resumeArchiveAfterRestart` are not advertised native settings in this version. `preferredSubtitleLanguage` is not an alias for `nativeSubtitleLanguage`, which configures the native HLS subtitle path. Preset and migration summaries must identify unsupported application settings.

## Validation

Check schema pagination, value validation and runtime consumers together. Protection tests cover source metadata, default/custom/disabled PIN policy, stale-source rejection and playback admission. Transport tests and host simulations are separate from testing the installed application on a television.

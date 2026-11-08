# USB folders in the native player

This native integration adds explicit removable-folder selection and optional verified filesystem reconnection to the Local provider. It does not copy, import, mount, format or download media.

## Prepare offline media

Put the videos on the drive before opening them in the player, and let the TV mount the drive. Local reads those existing files; importing a profile or Favorites does not transfer the videos or grant access to a folder on another device. Optional [artwork, descriptions and adjacent subtitles](local-assets.md) can live in the same authorized root without a network metadata service. Supported playback still depends on the TV decoder.

## Select a drive

1. Open **Providers → Local → Profiles** and choose **Browse USB drives**. When an existing USB profile is focused, the row identifies that target profile instead.
2. Choose a readable drive. This opens the folder picker; it does not yet change the selected source. A new target opens at the drive root, and an existing target opens at its exact retained folder.
3. Open a child folder, or use **Parent folder** and **Drive root** to move within the captured drive. **Use this folder** selects the displayed folder, including its videos in descendant folders. If a retained folder has disappeared, the picker reports an error and offers **Browse drive root** as an explicit action; it never substitutes the root silently.
4. Selection rechecks the current mount, captured root and displayed directory. The existing Saved transaction must succeed before the new source is installed. The normal progressive Local loader then emits the first playable catalogue row before the remaining bounded batches.

**Cancel** returns to Profiles without changing the accepted source, saved profile or Favorites. Back first cancels a pending folder read, retaining the previous page when one is available; from a loaded child it opens the parent; at the root it returns to the drive list. Loading always leaves Cancel available. A previous page remains visible while its replacement is read. A cancelled worker still occupies the existing catalogue slot until it actually exits; stale results cannot cancel or overwrite newer Local work. Standby closes the picker and requires explicit reopening.

After **Use this folder** has finished checking and dispatched the existing settings writer, the selection is no longer an uncommitted browse. Back or other navigation does not promise rollback of that write: its record may become durable, while the existing navigation/root checks prevent a stale acknowledgement from installing a live source.

The picker reads only the current directory. It initially examines up to 4,096 entries, including files, and displays at most 64 folder rows per page. **Load more** repeats the shallow read with a larger allowance, up to 500,000 examined entries; it does not reuse a partially buffered directory cursor. Previous/next page operate on the retained snapshot without filesystem access. Folder names are sorted and keep stable row identities when a larger result adds names. Changes in the live directory can still change the snapshot.

The retained snapshot is bounded to 4,096 folder names and 1 MiB of name bytes. A reached work or memory limit is shown explicitly, never as a complete empty result. At the final limit, select the current folder or cancel and enter the exact mounted folder in the Local profile's **Source address or folder**, then browse that drive for the same profile. A read has an eight-second cooperative budget; kernel calls themselves are not promised instantly interruptible. Symlinks are not entered, and a nested different device is not silently accepted as part of the selected mount.

Selecting a different folder for an existing USB profile preserves the profile/source IDs, name and alias, while the changed source follows the existing access-proof rules. Retaining the same folder at a new mount address uses the existing verified relocation path. Global Favorites and category documents are not replaced. A single-file Local source follows its separate exact-file workflow; this Folder picker and automatic Folder relocation do not apply to it.

Exiting the picker restores only its still-current interrupted catalogue plan. This also applies to a same-source category/Favorites save with `SaveNavigation::Preserve`; the existing catalogue/save concurrency and actual worker-slot guard are retained. The picker does not wait for that save acknowledgement or alter global save scheduling. A newer source operation or activating save supersedes the old plan.

## Reconnect and cold start

When the optional identity backend can verify a filesystem UUID and type during explicit selection, the existing Saved transaction retains that identity and the exact filesystem-relative folder. On a later cold start or reconnect, the active permanent Local profile or its configured Local-folder kiosk can locate that filesystem at a different mount address. The worker checks complete mount and identity observations before and after acquiring a fresh root handle. The saved folder must still exist inside the exposed filesystem view; the player never substitutes the volume root.

Automatic folder restoration runs through the existing catalog/startup lifecycle; the identity backend is not a hotplug monitor. For a saved, active non-kiosk USB folder with a verified stored identity, an unavailable initial observation can retry after 2 seconds, then 5 seconds, then at 10-second intervals within the original two-minute window. Each retry waits for the existing worker slots; it never catches up missed attempts in a loop. Stop, standby, another source or playback intent, or changed saved identity retires that schedule. Missing folders, denied access and ambiguous or changed volumes require explicit action. Kiosk keeps its separate recovery policy.

Retries themselves neither write Saved nor grant access. A successful observation still requires the ordinary fresh root/identity proof. If the same proven filesystem now has a different mount address, the existing verified relocation transaction must persist the new address before installation. After the finite window, activate the profile again or use **Browse USB drives**. A foreground refresh may request a new attempt only while the original restoration intent is still current.

The same source/profile IDs, relative-file bookmark and position are retained. Global Favorites resume and per-profile category preferences are rebased only through the verified relocation receipt and confirmed Saved transaction. A root-only kiosk reconnect does not select or load the permanent provider. Newer navigation or playback intent prevents a late reconnect result from activating playback.

This capability is conditional: a missing/inaccessible identity backend, unsupported filesystem mapping or a legacy path-only record still requires explicit selection. A mount pathname, kernel mount ID, device number or matching label is not a durable filesystem identity. A remembered `/tmp/usb/...` pathname alone never recreates an access grant. Simultaneously visible duplicate filesystem UUIDs are rejected. A filesystem UUID is not cryptographic hardware identity; a sole offline clone of that UUID cannot be distinguished by UUID alone. See [the identity backend and its limitations](usb-stable-identity.md).

During the current process lifetime, the existing Local source retains a private observation and open-root receipt. Refreshing that same source can retain it while both observations still match. A changed or absent source is reported through the existing Local inventory; stale worker results cannot revoke a newer source. Snapshot read errors, cancellation and oversize input are errors, not successful empty drive lists.

## Platform boundary

This guide describes the native filesystem integration, not USB access available to every webOS app. LG states that ordinary web apps cannot access USB storage, including for audio/video playback ([LG support](https://forum.webostv.developer.lge.com/t/service-to-see-usb-storage/2820), [2025 confirmation](https://forum.webostv.developer.lge.com/t/webos-internal-storage-and-usb/27902)). Packaging a web app or hosting its content elsewhere does not establish a supported USB folder picker. A JavaScript service's documented [filesystem access](https://webostv.developer.lge.com/develop/guides/js-service-basics) is not, by itself, a USB access grant.

The [webOS OSE storage API](https://www.webosose.org/docs/reference/ls2-api/com-webos-service-storageaccess/) documents drive discovery and directory listing under `storageaccess.operation`. That is an OSE contract, not evidence that a retail TV exposes or authorizes those methods. The [public webOS TV ACG list](https://webostv.developer.lge.com/develop/guides/acg-guide) does not list them. Do not copy an OSE permission declaration into a TV package and treat it as proof of access, or assume that Developer Mode or Homebrew installation grants unrestricted filesystem or private Luna access.

Qualification of the exact native build and firmware must distinguish an unsupported storage backend or mount layout, denied access to an observed drive/folder, and a missing or disconnected drive/folder. These are requirements for qualification and user-facing diagnostics, not a claim that every current build already presents a separate message for each case. Confirm enumeration, opening and playback from the original files without media copies, then reconnect and cold-start behavior. A successful Mac/host run or a mount visible to a shell does not establish access for the TV app process.

The LG native port reads the bounded `/proc/self/mountinfo` view visible to its own process and recognizes the `/tmp/usb/<disk>/<partition>` layout. It does not assume fixed disk letters, a particular filesystem, root access, an unrestricted mount namespace or permission merely because a mount is listed. Each candidate must also permit actual folder traversal. Other ports return unsupported unless they implement the neutral observation interface.

Developer-mode and Homebrew installations can expose different mounts and permissions. An empty list means that no usable candidate was observed in this process; it is not proof that no physical drive is connected. Automatic reconnection requires the documented identity backend and exact current evidence; it is not available on every firmware or mount layout. Universal host USB support and device-tested filesystem support are not claimed.

The retained root receipt is shared with the existing Local source owner. A folder catalogue task captures that live source before it is queued, then enumerates relative directories from the same held root descriptor. It does not reopen the profile pathname as a new root. A replaced named root cannot supply another folder's names, and loss or withdrawal prevents successful completion. The first playable catalogue row is still emitted before the remaining bounded batches. Video opening and decoding remain the responsibility of the native media pipeline.

The descriptor pins an opened directory, not immutable contents or a physical volume identity. Files can still change during a scan; nested-mount policy and final source admission remain the existing owner's responsibilities. Unreadable children retain their already readable prefix and report a warning when traversal finishes; root failure, depth/entry limits, cancellation and withdrawal remain incomplete scans.

## Verification status

The delivery packet contains source regression tests for the mount reader, explicit selection, folder containment, directory replacement, cancellation, stale completions, checkpoint rebinding and failed persistence. The original delivery packet was parsed and statically reviewed only. For a later release, use the qualification receipt for that exact source and binary; this guide does not certify the final 0.3.0 package. Mac/host fixtures use synthetic mount and identity observations with temporary folders. They do not establish real-LG USB permissions, reconnect behavior or playback. Compilation, test execution and physical-TV checks are separate integration gates and must not be inferred from the presence of tests.


## Selected saved folder at cold startup

For the first successfully decoded saved state, an enabled active Local USB folder with an exact remembered stable binding is verified before other remembered roots. This applies only to an empty, unloaded inventory without another playback or source intent. A saved Favorites view and kiosk policies retain their existing startup paths. Reading saved metadata grants no access: the ordinary Restore worker still verifies the device and opens the selected folder.

The selected attempt keeps priority on the existing catalogue worker and uses the finite delayed-mount retry policy. Missing or changed media does not start an unrelated all-root scan. Stop, standby, profile changes and new playback intent retire the preference; a later load is not another cold opportunity. The successful relocation transaction still uses the existing guarded Saved writer before installing the verified source.

After installation, the selected held-folder scan can deliver its first media without validating unrelated roots. Those rows remain remembered intents until later existing explicit operations or lifecycle revalidation. No new background scanner is created.

When an explicit remote folder selection succeeds while that initial attempt is waiting, the verified selected folder completes the previously empty inventory. The selection remains temporary: it does not change the saved profile or begin playback. A following handle command can use the selected folder without first checking unrelated remembered roots. Rejected selections and other inventory states retain their existing behavior.

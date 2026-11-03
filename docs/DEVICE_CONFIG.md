# Persistent device configuration and public firmware

Verified on hardware: HTTPS OTA and health confirmation preserve the stored
configuration. Forced rollback and hardware power-loss testing remain pending.

## Decision

Store each device's Wi-Fi and MQTT settings in a dedicated flash data partition.
Load them at boot into bounded RAM storage, and publish the same credential-free
application image for every device using this board configuration.

Provision settings over local USB from a private file under the ignored `local/`
directory; firmware builds never read it. A web setup portal and remote
credential changes are deferred.

## Partition layout

`device_config` uses the last 64 KiB of the 4 MiB flash, after `ota_1`:

```csv
device_config, data, 0x06, 0x3f0000, 0x10000
```

This does not shrink either OTA slot. The partition uses the known Undefined
subtype (0x06) because the partition parser only supports known subtypes. Its
contents are not ESP-IDF NVS. At boot the firmware checks the type, offset and
size of this and the OTA partitions; a mismatch disables configuration and OTA.

OTA writes only inactive app slots and OTA metadata. Routine USB firmware
flashes preserve `device_config`; full-chip erase and an explicit local settings
reset clear it.

## Storage and update transaction

Use `sequential-storage` 8.0.2 with one map entry containing the entire JSON
snapshot. This provides CRC checks, power-failure recovery and page management.
`ekv` is Embassy's alternative transactional key/value store; a multi-key
database is unnecessary for this single snapshot.

The bounded schema contains Wi-Fi SSID/authentication/password, MQTT broker/port/
username/password, schema version and a monotonically increasing generation.
Unknown schemas and malformed settings are rejected, not replaced with defaults.
A major storage-library upgrade requires an explicit on-flash format migration.

Flash operations use `YieldingAsync<BlockingAsync<&mut FlashStorage>>` with the
NOR traits, so the store uses explicit erase/program operations rather than
read-modify-erase-write. Yielding occurs between operations; individual flash
operations remain synchronous. One flash owner serializes saves and OTA.

Configuration is updated through firmware commands over USB, not by generating
or writing the store's private on-disk format on the host. After a save or erase
attempt the device responds with metadata only and reboots, so active settings
always match the stored ones even if a failed write committed.

## Firmware rules

- `DeviceConfig` has no `Debug` implementation; credentials never reach logs.
- Configuration is loaded before networking starts. Afterwards `flash.rs` is
  the only `FlashStorage` owner and serializes configuration writes with OTA.
- With missing, corrupt, or unsupported configuration, local LED/button
  operation stays available and serial output reports that configuration is
  required. No setup AP or remote provisioning endpoint starts automatically.
- A pending trial image still times out and resets into rollback after two
  minutes when configuration is unavailable.
- Status reports configuration presence, schema and generation only. Never
  publish credentials, the raw record, or a digest of secret configuration in
  MQTT status, public manifests, build IDs, or diagnostic output.

## Local provisioning tool

The public CLI is `tools/provision.py`:

```sh
uv run tools/provision.py status --port PORT
uv run tools/provision.py provision --port PORT --config local/device.json
uv run tools/provision.py erase-config --port PORT
```

The private input may be JSON or YAML. The CLI sends bounded JSON over USB;
the firmware validates and persists it. Responses contain metadata only.
Credentials are not passed as command-line arguments. Keep private input files
under the ignored `local/` directory.

`erase-config` is an explicit local reset. Online credential changes, setup APs,
and BLE provisioning are deferred.

## Schema compatibility and rollback

Configuration outlives the application slot that wrote it. Version 1 OTA images
must read the same version 1 format without rewriting it during trial boot.

Additive optional fields can retain old-reader compatibility through field IDs.
An incompatible future migration needs a separate design: retain a record the
rollback image can read until the new firmware is confirmed. Never erase the
only old-format copy before confirmation. Unknown/unsupported formats must not
be silently replaced with empty defaults.

## Publication and versioning

Public releases use a UTC timestamp, for example `20261102T194500Z`. Keep the
full Git SHA and dirty state in a private local build record, not in public
versions or manifests. The image filename also identifies its compiled hardware
profile, for example `m5stack-nanoc6-sk6812-grb-60led`.

The workflow CLI and its bucket/project/MQTT configuration live in the ignored
`local/` directory, outside version control. Do not add repository links, bucket identifiers, Git SHAs, or GCP
publishing configuration to public artifacts or GitHub Actions. Timestamp
ordering is not a security anti-rollback mechanism.

Publish immutable objects under the requested prefix:

```text
gs://BUCKET/firmware/esp32-led-mqtt/VERSION/firmware.bin
gs://BUCKET/firmware/esp32-led-mqtt/VERSION/manifest.json
```

The manifest contains the exact HTTPS image URL, build/version, UTC build time,
chip/application, hardware profile, required config schema, byte count,
CRC32, SHA-256, and test status. Include an `ota_command` object containing only
the fields accepted by the firmware command parser; publication metadata is not
sent as extra command fields. Upload the binary first and the manifest last,
with a generation-zero precondition to prevent overwrites. Verify anonymous
HTTPS access, size, and digest. Keep publication independent from issuing an
MQTT update command.

Use `storage.googleapis.com` in the firmware host allowlist. A mutable `latest`
pointer is deferred until we have a policy for promoting hardware-tested builds;
an untested dirty build must not automatically become the recommended update.

## Validation

- Host tests: malformed/oversized records, unsupported schemas, invalid fields,
  generations, partial writes, corrupt headers/payloads/commit words, and fallback
  to the previous complete record.
- Simulated interruption after each erase/program step; detect attempts to erase
  the only valid record before its replacement commits.
- Provisioning tests: wrong board/layout, blank flash, read-back failure, and
  interrupted USB writes; recovery must retain the old record or require a
  deliberate local reprovision, never accept partial credentials.
- Public-build check: build with distinctive canary credentials in an ignored
  local `secrets.yaml`, then assert those bytes are absent from both app ELF and
  OTA binary. Inspect the dependency/build graph to confirm the file is no longer
  read. Do not upload a merged full-flash backup containing the config partition.
- Hardware: successful stored-config boot, OTA in both directions, trial rollback,
  routine USB app flash preserving config, missing/corrupt config recovery, and
  power loss during a settings change.

This separation keeps credentials out of published images. It does not protect
against someone physically reading unencrypted device flash. Flash encryption,
key provisioning, and eFuse changes are a separate future decision.

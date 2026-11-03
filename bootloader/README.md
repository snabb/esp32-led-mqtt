# Rollback bootloader

OTA rollback needs an ESP-IDF bootloader built with
`CONFIG_BOOTLOADER_APP_ROLLBACK_ENABLE`. espflash's bundled bootloader doesn't
have it. Build one with:

```sh
tools/build-ota-bootloader.sh
```

The script needs Docker and network access. It runs ESP-IDF 6.1 from
`espressif/idf@sha256:81893c71bb5e570088901f21def8684c25cd2a9020281bd01b843a7655edb18c`
(ESP-IDF revision `fff9895c82d744c7237be8847347bdd1b07c6643`). The container
mounts only a fresh `target/ota-bootloader.*` directory, not the repository.
Set `IDF_IMAGE` to use a different image.

[`sdkconfig.defaults`](sdkconfig.defaults) selects ESP32-C6, rollback, DIO
80 MHz 4 MiB flash and a partition table at `0x8000`. The script checks that
rollback is enabled and that the bootloader fits below `0x8000`. The partition
table is read from flash at boot, so the ESP-IDF project's own default table
doesn't matter.

## Output

All of these are ignored by Git:

| File | Contents |
|---|---|
| `esp32c6-rollback.bin` | The bootloader. [`espflash.toml`](../espflash.toml) points to it. |
| `sdkconfig.generated` | Full resolved configuration |
| `container-image.txt`, `idf-revision.txt`, `idf-submodules.txt` | Provenance |
| `SHA256SUMS` | Digests of the binary and configuration |
| `build.log` | Full build output |

A build on 2026-11-02 produced a 22,224-byte bootloader (10,544 bytes free
below the partition table) with SHA-256
`4c1b271b88cb23627fc6d6978afee6e1f9d6b7062a8c8cd5de75c6febd8afad3`.
That records one build. Later builds aren't guaranteed to be byte-identical.

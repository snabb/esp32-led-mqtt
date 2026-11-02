# Rollback bootloader

Run `tools/build-ota-bootloader.sh` from the repository. It uses ESP-IDF 6.1 in
`espressif/idf@sha256:81893c71bb5e570088901f21def8684c25cd2a9020281bd01b843a7655edb18c`.
The source revision in that image is
`fff9895c82d744c7237be8847347bdd1b07c6643`.

`sdkconfig.defaults` enables application rollback for ESP32-C6, DIO/80 MHz/4 MiB
flash and a partition table at 0x8000. The generated bootloader must fit below
that boundary. The application partition CSV is read from flash at runtime; the
bootloader build project's default table is not the deployment table.

A build on 2026-11-02 produced a 22,224-byte bootloader, leaving 10,544 bytes
before the partition table. Its SHA-256 was
`4c1b271b88cb23627fc6d6978afee6e1f9d6b7062a8c8cd5de75c6febd8afad3`.
This records that build, not a guarantee of byte-identical future builds.

The script writes the generated `sdkconfig`, source/submodule revisions, image
digest and `SHA256SUMS` alongside `esp32c6-rollback.bin`. Full build output is copied to
`bootloader/build.log`; each run uses a fresh `target/ota-bootloader.*` directory. Set `IDF_IMAGE` to the recorded digest to
repeat a particular build if overriding the default image.

Do not replace this artifact with espflash's default bootloader: it does not
provide the rollback behavior expected by the prototype.

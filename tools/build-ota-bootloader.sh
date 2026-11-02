#!/usr/bin/env bash
set -euo pipefail
repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
image=${IDF_IMAGE:-espressif/idf@sha256:81893c71bb5e570088901f21def8684c25cd2a9020281bd01b843a7655edb18c}
mkdir -p "$repo/target"
work=$(mktemp -d "$repo/target/ota-bootloader.XXXXXX")
mkdir -p "$work/main"
cp "$repo/bootloader/sdkconfig.defaults" "$work/sdkconfig.defaults"
cat > "$work/CMakeLists.txt" <<'CMAKE'
cmake_minimum_required(VERSION 3.16)
include($ENV{IDF_PATH}/tools/cmake/project.cmake)
project(ota_bootloader)
CMAKE
cat > "$work/main/CMakeLists.txt" <<'CMAKE'
idf_component_register(SRCS "main.c")
CMAKE
cat > "$work/main/main.c" <<'C'
void app_main(void) {}
C
# Only the disposable build context is mounted; private firmware credentials stay outside.
docker pull "$image"
digest=$(docker image inspect "$image" --format '{{index .RepoDigests 0}}')
printf '%s\n' "$digest" > "$work/container-image.txt"
docker run --rm --user "$(id -u):$(id -g)" -e HOME=/tmp \
    -v "$work:/project" -w /project "$digest" bash -c '
set -euo pipefail
idf.py set-target esp32c6
idf.py bootloader
grep -qx CONFIG_BOOTLOADER_APP_ROLLBACK_ENABLE=y sdkconfig
python "$IDF_PATH/components/partition_table/check_sizes.py" --offset 0x8000 bootloader 0x0 build/bootloader/bootloader.bin
git -C "$IDF_PATH" rev-parse HEAD > idf-revision.txt
git -C "$IDF_PATH" submodule status --recursive > idf-submodules.txt
' 2>&1 | tee "$work/build.log"
cp "$work/build/bootloader/bootloader.bin" "$repo/bootloader/esp32c6-rollback.bin"
cp "$work/sdkconfig" "$repo/bootloader/sdkconfig.generated"
cp "$work/build.log" "$repo/bootloader/build.log"
cp "$work/container-image.txt" "$work/idf-revision.txt" "$work/idf-submodules.txt" "$repo/bootloader/"
(cd "$repo/bootloader" && sha256sum esp32c6-rollback.bin sdkconfig.defaults sdkconfig.generated > SHA256SUMS)
printf 'Bootloader and provenance written to %s/bootloader\n' "$repo"

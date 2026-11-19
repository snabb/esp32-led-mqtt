#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = ["aiocoap[oscore]==0.4.17", "cbor2==6.3.0"]
# ///
"""Control a device over CoAP, authenticated with EDHOC and protected with OSCORE.

Keys live under local/coap/. `keygen` makes this client's key; `tools/provision.py
--coap-client` gives its public part to a device, and saves the device's public key where
this tool looks for it. See docs/COAP.md.
"""

import argparse
import asyncio
import hashlib
import ipaddress
import json
import os
import secrets
import socket
import sys
import zlib
from pathlib import Path

import aiocoap
import cbor2
from aiocoap.edhoc import CoseKeyForEdhoc, EdhocCredentials

KEY_DIR = Path(__file__).resolve().parent.parent / "local/coap"
# Where the build ID sits in an ESP image: the app descriptor's version field.
BUILD_ID_RANGE = (48, 80)
TRIAL_SECONDS = 150
STAGE_SECONDS = 30
# `confirm_boot` reports these as `disabled` when a boot is unhealthy. Others just mean this
# image can't download updates.
UNHEALTHY_BOOT = {
    "bootloader_state",
    "initial_metadata",
    "partition_layout",
    "partition_table",
    "missing_partition",
    "otadata_erase",
    "unknown_image_state",
    "unknown_running_slot",
}


class CoapError(Exception):
    pass


def keygen(path):
    """Create this client's key pair. An existing key is never replaced."""
    key_file, ccs_file = path, path.with_suffix(".ccs")
    if key_file.exists():
        raise CoapError(f"{key_file} already exists")
    path.parent.mkdir(parents=True, exist_ok=True)
    key = CoseKeyForEdhoc.generate(key_file)
    # Random, because a device can't tell apart two clients with the same key ID.
    kid = secrets.token_bytes(4)
    ccs_file.write_bytes(
        cbor2.dumps(key.as_ccs(kid=kid, subject=None)[14], canonical=True)
    )
    print(f"Private key: {key_file}")
    print(f"Public key, for devices: {ccs_file}")


def resolve_all(host):
    """Returns candidate URI authorities for `host`: an IPv6 literal, bracketed or not, or else a
    name or IPv4 address with an optional port. A name resolves to its IPv4 address if it
    has one, else to IPv6; a device doesn't need the name in requests.

    IPv6 addresses come out in aiocoap's normal form, as it looks credentials up by the
    normalized URI, and zones as a bare `%ifname`, which its UDP transport expects."""
    if host.startswith("["):
        address, _, rest = host[1:].partition("]")
        return [f"[{ipaddress.IPv6Address(address)}]{rest}"]
    try:
        return [f"[{ipaddress.IPv6Address(host)}]"]
    except ValueError:
        pass
    name, separator, port = host.partition(":")
    authorities = {}
    error = None
    for family in (socket.AF_INET, socket.AF_INET6):
        try:
            entries = socket.getaddrinfo(name, None, family, socket.SOCK_DGRAM)
        except socket.gaierror as exc:
            error = exc
            continue
        for entry in entries:
            sockaddr = entry[4]
            if family == socket.AF_INET:
                authority = sockaddr[0]
            else:
                address = ipaddress.IPv6Address(sockaddr[0].split("%")[0])
                zone = f"%{socket.if_indextoname(sockaddr[3])}" if sockaddr[3] else ""
                authority = f"[{address}{zone}]"
            authorities[f"{authority}{separator}{port}"] = None
    if not authorities and error is not None:
        raise error
    return list(authorities)


def resolve(host):
    return resolve_all(host)[0]


class Device:
    def __init__(self, args):
        self.hostname = args.host
        self.host = resolve(args.host)
        host_part = args.host.removesuffix(".local")
        self.device_ccs_file = args.device_ccs or KEY_DIR / f"{host_part}.ccs"
        self.key_file = args.client_key
        self.context = None

    def uri(self, path):
        return f"coap://{self.host}{path}"

    async def __aenter__(self):
        if not self.key_file.exists():
            raise CoapError(f"no client key at {self.key_file}; run keygen")
        if not self.device_ccs_file.exists():
            raise CoapError(
                f"no device key at {self.device_ccs_file}; see --device-ccs"
            )
        key = CoseKeyForEdhoc.from_file(self.key_file)
        own = cbor2.loads(self.key_file.with_suffix(".ccs").read_bytes())
        kid = own[8][1][2]
        self.credentials = EdhocCredentials(
            suite=2,
            method=3,
            own_cred_style="by-key-id",
            own_cred=key.as_ccs(kid=kid, subject=None),
            private_key=key.secret_to_map(),
            peer_cred={14: cbor2.loads(self.device_ccs_file.read_bytes())},
        )
        await self.connect(self.host)
        return self

    async def __aexit__(self, *_):
        if self.context is not None:
            await self.context.shutdown()

    async def connect(self, authority):
        if self.context is not None:
            await self.context.shutdown()
            self.context = None
        self.context = await aiocoap.Context.create_client_context()
        self.host = authority
        self.context.client_credentials[self.uri("/*")] = self.credentials

    def message(self, code, path, payload=b"", **options):
        return aiocoap.Message(
            code=code, uri=self.uri(path), payload=payload, **options
        )

    async def request(self, code, path, payload=b""):
        response = await self.context.request(
            self.message(code, path, payload)
        ).response
        if not response.code.is_successful():
            detail = response.payload.decode(errors="replace")
            raise CoapError(f"{path}: {response.code} {detail}".rstrip())
        return response.payload

    async def get(self, path):
        return await self.request(aiocoap.GET, path)

    async def put(self, path, payload):
        return await self.request(aiocoap.PUT, path, payload)

    async def post(self, path, payload=b""):
        return await self.request(aiocoap.POST, path, payload)


def parse_logs(payload):
    header, _, text = payload.partition(b"\n")
    start, _, following = header.partition(b" ")
    return int(start), int(following), text


class LogPrinter:
    """Prints log text in order, whichever way it arrives, and says what was lost."""

    def __init__(self):
        self.cursor = None

    def show(self, start, following, text):
        if self.cursor is not None and following < self.cursor:
            print("-- device restarted --", flush=True)
            self.cursor = None
        if self.cursor is not None and start > self.cursor:
            print(f"-- dropped {start - self.cursor} bytes --", flush=True)
        begin = 0 if self.cursor is None else max(self.cursor - start, 0)
        sys.stdout.write(text[begin:].decode(errors="replace"))
        sys.stdout.flush()
        self.cursor = following


async def read_logs_until(device, printer, end):
    """Reads from the printer's cursor up to the offset `end`. Stopping only when there's
    nothing new would never end, because reading can itself make a device log."""
    while printer.cursor is None or printer.cursor < end:
        payload = await device.get(f"/logs?from={printer.cursor or 0}")
        start, following, text = parse_logs(payload)
        printer.show(start, following, text)
        if not text:
            return


async def show_logs(device, follow):
    printer = LogPrinter()
    _, end, _ = parse_logs(await device.get("/logs"))
    await read_logs_until(device, printer, end)
    if not follow:
        return
    request = device.context.request(device.message(aiocoap.GET, "/logs", observe=0))
    await request.response
    async for notification in request.observation:
        start, following, text = parse_logs(notification.payload)
        # A gap means notifications were coalesced or lost, so fetch what's missing.
        if printer.cursor is not None and printer.cursor < start <= following:
            await read_logs_until(device, printer, following)
        else:
            printer.show(start, following, text)


def fingerprint_of(ccs_file):
    return hashlib.sha256(ccs_file.read_bytes()).hexdigest()[:8]


async def client_fingerprints(device):
    # A reply only means the change was accepted, and saving it follows. Allow for that.
    await asyncio.sleep(1)
    return json.loads(await device.get("/coap/clients"))


async def clients(device, args):
    if args.action == "list":
        print((await device.get("/coap/clients")).decode())
        return
    if args.action == "add":
        await device.post("/coap/clients", args.client.read_bytes().hex().encode())
        fingerprint = fingerprint_of(args.client)
        if fingerprint not in await client_fingerprints(device):
            raise CoapError(f"{fingerprint} wasn't saved")
        print(f"Added {fingerprint}")
        return
    fingerprint = args.client
    if Path(fingerprint).exists():
        fingerprint = fingerprint_of(Path(fingerprint))
    response = await device.context.request(
        device.message(aiocoap.DELETE, f"/coap/clients?fp={fingerprint}")
    ).response
    if not response.code.is_successful():
        detail = response.payload.decode(errors="replace")
        raise CoapError(f"could not remove {fingerprint}: {response.code} {detail}")
    if fingerprint in await client_fingerprints(device):
        raise CoapError(f"{fingerprint} wasn't removed")
    print(f"Removed {fingerprint}")
    if args.no_reboot:
        print("Sessions it already has last until the device reboots.")
    else:
        # Removal only stops new handshakes, so end any session the key still has.
        await asyncio.sleep(1)
        await device.post("/reboot")
        print("Rebooting, to end any session it still has.")


def image_build_id(image):
    field = image[BUILD_ID_RANGE[0] : BUILD_ID_RANGE[1]]
    return field.split(b"\0", 1)[0].decode()


async def wait_for_build(device, build, *, confirm_host=None):
    """Polls until the image has passed its trial. These requests are themselves what
    confirms it when no broker is reachable, but its other health checks have to pass too."""
    loop = asyncio.get_running_loop()
    deadline = loop.time() + TRIAL_SECONDS
    original_host = device.host
    was_running = False
    while (remaining := deadline - loop.time()) > 0:
        await asyncio.sleep(min(5, remaining))
        try:
            async with asyncio.timeout_at(min(deadline, loop.time() + 5)):
                candidates = await asyncio.to_thread(
                    resolve_all,
                    confirm_host or device.hostname,
                )
        except OSError:
            continue
        # Prefer the last confirmed address after the target starts responding.
        if was_running and device.host in candidates:
            candidates.remove(device.host)
            candidates.insert(0, device.host)
        for authority in candidates:
            status = await read_trial_status(
                device,
                authority,
                deadline,
                fresh=not was_running,
                timeout=5 if len(candidates) > 1 else 20,
            )
            if status is None:
                continue
            state, detail = status.get("state"), status.get("detail")
            if status.get("running_build") != build:
                if was_running:
                    raise CoapError(f"{build} failed its trial and rolled back")
                if state == "failed":
                    raise CoapError(f"update failed: {detail}")
                continue
            was_running = True
            if state == "disabled" and detail in UNHEALTHY_BOOT:
                raise CoapError(f"{build} booted unhealthy: {detail}")
            if state != "trial":
                print(f"Running {build}: {state}")
                return
            break
    if confirm_host is not None:
        status = await read_trial_status(
            device, original_host, loop.time() + 5, fresh=True
        )
        if (
            status
            and status.get("running_build") != build
            and status.get("state") == "ready"
        ):
            raise CoapError(
                f"{build} failed its trial and rolled back to the previous image"
            )
    raise CoapError(f"{build} hasn't passed its trial after {TRIAL_SECONDS} s")


async def read_trial_status(device, authority, deadline, *, fresh, timeout=20):
    try:
        async with asyncio.timeout_at(
            min(deadline, asyncio.get_running_loop().time() + timeout)
        ):
            if fresh or device.context is None or device.host != authority:
                await device.connect(authority)
            return json.loads(await device.get("/firmware"))
    except (aiocoap.error.Error, ValueError, CoapError, OSError):
        return None


async def push(device, image, *, confirm_host=None):
    build = image_build_id(image)
    manifest = {
        "size": len(image),
        "crc32": zlib.crc32(image),
        "sha256": hashlib.sha256(image).hexdigest(),
        "target": "esp32c6",
        "build": build,
    }
    await device.put(
        "/firmware/manifest", json.dumps(manifest, separators=(",", ":")).encode()
    )
    print(f"Uploading {build}: {len(image)} bytes")
    started = asyncio.get_running_loop().time()
    await device.put("/firmware/image", image)
    print(f"Uploaded in {asyncio.get_running_loop().time() - started:.1f} s")
    await wait_for_build(device, build, confirm_host=confirm_host)


async def save_config(device, settings):
    resource = "/config/stored"
    previous = json.loads(await device.get(resource))["generation"]
    await device.put(resource, settings)
    try:
        async with asyncio.timeout(STAGE_SECONDS):
            while True:
                status = json.loads(await device.get(resource))
                if status["status"] == "failed":
                    raise CoapError("Settings save failed")
                if status["status"] == "saved" and status["generation"] > previous:
                    print(
                        f"Settings saved without reboot at generation {status['generation']}"
                    )
                    return
                await asyncio.sleep(0.25)
    except TimeoutError:
        raise CoapError("Timed out waiting for settings to be saved") from None


async def run(args):
    async with Device(args) as device:
        match args.command:
            case "get":
                print((await device.get(args.path)).decode(errors="replace"))
            case "info" | "light-state" | "effects" | "firmware" | "temperature":
                path = {"light-state": "/light"}.get(args.command, f"/{args.command}")
                print((await device.get(path)).decode())
            case "light":
                await device.put("/light", args.json.encode())
            case "speed":
                await device.put("/speed", str(args.value).encode())
            case "variation":
                await device.put("/variation", str(args.value).encode())
            case "logs":
                await show_logs(device, args.follow)
            case "config-get":
                print((await device.get("/config")).decode())
            case "config-put":
                settings = args.file.read_bytes()
                if args.no_reboot:
                    await save_config(device, settings)
                else:
                    print("Applying settings and rebooting.")
                    await device.put("/config", settings)
            case "clients":
                await clients(device, args)
            case "ota":
                await device.post("/firmware/update", args.file.read_bytes())
            case "push":
                await push(
                    device, args.image.read_bytes(), confirm_host=args.confirm_host
                )
            case "reboot":
                await device.post("/reboot")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--host", help="for example esp32-led-3ef8d0.local")
    parser.add_argument("--device-ccs", type=Path, help="the device's public key")
    parser.add_argument("--client-key", type=Path, default=KEY_DIR / "client.key")
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("keygen", help="create this client's key")
    commands.add_parser("get", help="read any resource").add_argument("path")
    for name in ("info", "light-state", "effects", "firmware", "temperature", "reboot"):
        commands.add_parser(name)
    commands.add_parser("light", help="send a light command").add_argument("json")
    commands.add_parser("speed").add_argument("value", type=int)
    commands.add_parser("variation").add_argument("value", type=int)
    logs = commands.add_parser("logs", help="read recent logs")
    logs.add_argument(
        "-f", "--follow", action="store_true", help="keep printing new lines"
    )
    commands.add_parser("config-get", help="read settings, without secrets")
    config_put = commands.add_parser("config-put", help="replace settings and reboot")
    config_put.add_argument("file", type=Path)
    config_put.add_argument(
        "--no-reboot", action="store_true", help="save for the next boot"
    )
    client_commands = commands.add_parser("clients", help="manage trusted client keys")
    client_actions = client_commands.add_subparsers(dest="action", required=True)
    client_actions.add_parser("list")
    client_actions.add_parser("add").add_argument(
        "client", type=Path, help="its .ccs file"
    )
    remove = client_actions.add_parser("remove")
    remove.add_argument("client", help="a fingerprint or a .ccs file")
    remove.add_argument("--no-reboot", action="store_true")
    commands.add_parser("ota", help="start an HTTPS update").add_argument(
        "file", type=Path
    )
    push_command = commands.add_parser("push", help="upload an image")
    push_command.add_argument("image", type=Path)
    push_command.add_argument("--confirm-host", help="hostname or address after reboot")
    args = parser.parse_args()
    if args.command == "keygen":
        keygen(args.client_key)
        return
    if not args.host:
        parser.error("--host is required")
    asyncio.run(run(args))


if __name__ == "__main__":
    try:
        main()
    except (CoapError, aiocoap.error.Error, OSError, ValueError) as error:
        print(f"Error: {error}", file=sys.stderr)
        sys.exit(1)
    except KeyboardInterrupt:
        os._exit(0)

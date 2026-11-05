#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = ["aiocoap[oscore]==0.4.17", "cbor2==6.2.0.post2"]
# ///
"""Control a device over CoAP, authenticated with EDHOC and protected with OSCORE.

Keys live under local/coap/. `keygen` makes this client's key; `tools/provision.py
--coap-client` gives its public part to a device, and saves the device's public key where
this tool looks for it. See docs/COAP.md.
"""

import argparse
import asyncio
import hashlib
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


def resolve_ipv4(host):
    """Devices only have IPv4. Left to aiocoap, a lookup by name waits about 10 s for an
    IPv6 mDNS answer that won't come. A device doesn't need the name in requests."""
    name, separator, port = host.partition(":")
    address = socket.getaddrinfo(name, None, socket.AF_INET)[0][4][0]
    return f"{address}{separator}{port}"


class Device:
    def __init__(self, args):
        self.host = resolve_ipv4(args.host)
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
        self.context = await aiocoap.Context.create_client_context()
        self.context.client_credentials[self.uri("/*")] = EdhocCredentials(
            suite=2,
            method=3,
            own_cred_style="by-key-id",
            own_cred=key.as_ccs(kid=kid, subject=None),
            private_key=key.secret_to_map(),
            peer_cred={14: cbor2.loads(self.device_ccs_file.read_bytes())},
        )
        return self

    async def __aexit__(self, *_):
        await self.context.shutdown()

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


async def clients(device, args):
    if args.action == "list":
        print((await device.get("/coap/clients")).decode())
        return
    if args.action == "add":
        await device.post("/coap/clients", args.client.read_bytes().hex().encode())
        print(f"Added {fingerprint_of(args.client)}")
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


async def wait_for_build(device, build):
    """Polls until the image runs. Each authenticated request also confirms its trial."""
    deadline = asyncio.get_running_loop().time() + TRIAL_SECONDS
    while asyncio.get_running_loop().time() < deadline:
        await asyncio.sleep(5)
        try:
            status = json.loads(await device.get("/firmware"))
        except (aiocoap.error.Error, ValueError, CoapError):
            continue
        if status.get("running_build") == build:
            print(f"Running {build}: {status.get('state')}")
            return
        if status.get("state") == "failed":
            raise CoapError(f"update failed: {status.get('detail')}")
    raise CoapError(
        f"{build} isn't running after {TRIAL_SECONDS} s; it may have rolled back"
    )


async def push(device, image_file):
    image = image_file.read_bytes()
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
    await wait_for_build(device, build)


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
            case "logs":
                await show_logs(device, args.follow)
            case "config-get":
                print((await device.get("/config")).decode())
            case "config-put":
                print(
                    "Applying settings and rebooting. Wrong Wi-Fi settings need USB to fix."
                )
                await device.put("/config", args.file.read_bytes())
            case "clients":
                await clients(device, args)
            case "ota":
                await device.post("/firmware/update", args.file.read_bytes())
            case "push":
                await push(device, args.image)
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
    logs = commands.add_parser("logs", help="read recent logs")
    logs.add_argument(
        "-f", "--follow", action="store_true", help="keep printing new lines"
    )
    commands.add_parser("config-get", help="read settings, without secrets")
    config_put = commands.add_parser("config-put", help="replace settings and reboot")
    config_put.add_argument("file", type=Path)
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
    commands.add_parser("push", help="upload an image").add_argument("image", type=Path)
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

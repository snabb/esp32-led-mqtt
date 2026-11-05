#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = ["pyserial==3.5", "PyYAML==6.0.3"]
# ///
"""Provision persistent ESP32 device settings over USB without rebuilding firmware."""

import argparse
import json
import os
import sys
import time
from pathlib import Path


def save_device_credential(reply):
    """Keep the device's public CoAP key where tools/coap.py looks for it."""
    credential, device = reply.get("coap_ccs"), reply.get("device")
    if credential and device:
        path = Path(__file__).resolve().parent.parent / "local/coap" / f"{device}.ccs"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(bytes.fromhex(credential))
        print(f"Saved the device's CoAP credential to {path}")


def usb(config, port, operation, timeout=30):
    import serial

    request = {"op": operation, "id": os.urandom(8).hex()}
    if operation == "configure":
        request["config"] = config
    frame = b"@CONFIG " + json.dumps(request, separators=(",", ":")).encode() + b"\n"
    if len(frame) > 4224:
        raise ValueError("configuration frame too large")
    with serial.Serial() as link:
        link.port, link.baudrate, link.timeout = port, 115200, 0.2
        link.dsrdtr = link.rtscts = False
        link.write_timeout = 5
        link.open()
        deadline = time.monotonic() + timeout
        # Opening native USB may reset the board; wait for its app to service status.
        time.sleep(1)
        link.write(frame)
        link.flush()
        while time.monotonic() < deadline:
            line = link.readline()
            if not line.startswith(b"@CONFIG "):
                continue
            try:
                reply = json.loads(line[8:])
            except (ValueError, UnicodeError):
                continue
            if reply.get("id") not in (request["id"], ""):
                continue
            if reply.get("status") not in (
                "configured",
                "unconfigured",
                "saved",
                "erased",
            ):
                raise RuntimeError(
                    f"provisioning failed: {reply.get('status', 'invalid response')}"
                )
            print(json.dumps(reply))
            save_device_credential(reply)
            return
    raise TimeoutError("no USB configuration response")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=("status", "provision", "erase-config"))
    parser.add_argument("--port", required=True)
    parser.add_argument(
        "--config", type=Path, help="Private JSON or YAML settings file"
    )
    parser.add_argument(
        "--coap-client",
        type=Path,
        action="append",
        default=[],
        help="Client CoAP credential (.ccs) to trust, replacing any stored ones",
    )
    parser.add_argument("--timeout", type=float, default=30)
    args = parser.parse_args()
    config = None
    if args.operation == "provision":
        if args.config is None:
            parser.error("provision requires --config")
        import yaml

        try:
            config = yaml.safe_load(args.config.read_text())
        except yaml.YAMLError:
            raise ValueError("invalid configuration file") from None
        if not isinstance(config, dict):
            raise ValueError("configuration must be a mapping")
        if args.coap_client:
            config["coap_clients"] = [
                path.read_bytes().hex() for path in args.coap_client
            ]
    usb(
        config,
        args.port,
        {"status": "status", "provision": "configure", "erase-config": "erase"}[
            args.operation
        ],
        args.timeout,
    )


if __name__ == "__main__":
    try:
        main()
    except (ValueError, TypeError, OSError, RuntimeError) as error:
        print(f"Error: {error}", file=sys.stderr)
        sys.exit(1)

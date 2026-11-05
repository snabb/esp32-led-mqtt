#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = ["paho-mqtt==2.2.0"]
# ///
"""Move one device's retained MQTT topics from esp32_led_mqtt_XXXXXX to esp32-led-XXXXXX.

Run `prepare` before updating to an image with the new name and `finish` after it. See
docs/MQTT.md. The password is read from the MQTT_PASSWORD environment variable.
"""

import argparse
import os
import re
import ssl
import sys
import threading
import time
from pathlib import Path

import paho.mqtt.client as mqtt

# Retained messages follow the SUBACK at once, so this only bounds the wait for absent topics.
READ_SECONDS = 2.0
ACK_SECONDS = 10.0
STATE_TOPICS = ("light/state", "effect_speed/state")
OTHER_TOPICS = ("status", "temperature/state", "firmware/state", "ota/status")


def old_discovery(suffix):
    device = f"esp32_led_mqtt_{suffix}"
    return (
        f"homeassistant/light/{device}/config",
        f"homeassistant/number/{device}_effect_speed/config",
        f"homeassistant/sensor/{device}_temperature/config",
        f"homeassistant/sensor/{device}_firmware/config",
    )


def connect(args):
    client = mqtt.Client(mqtt.CallbackAPIVersion.VERSION2, protocol=mqtt.MQTTv311)
    if args.username or "MQTT_PASSWORD" in os.environ:
        client.username_pw_set(args.username or "", os.environ.get("MQTT_PASSWORD", ""))
    if args.tls:
        roots = Path(__file__).resolve().parent.parent / "src/roots.pem"
        client.tls_set(ca_certs=str(roots), tls_version=ssl.PROTOCOL_TLS_CLIENT)
    ready = threading.Event()
    client.on_connect = lambda c, u, f, reason, p: ready.set() if reason == 0 else None
    client.connect(args.broker, args.port, keepalive=30)
    client.loop_start()
    if not ready.wait(10):
        raise TimeoutError("could not connect to the broker")
    return client


def read_retained(client, topics):
    """Return {topic: payload} for those topics that have a retained message.

    Absent from the result means the broker has none. A refused or unanswered subscription
    is an error, because treating it as absent would lose the state being migrated.
    """
    found = {}
    acknowledged = threading.Event()
    refused = []

    def on_subscribe(c, userdata, mid, reasons, properties):
        refused.extend(reason for reason in reasons if reason.is_failure)
        acknowledged.set()

    # paho calls these on its network thread, one message at a time.
    client.on_subscribe = on_subscribe
    client.on_message = lambda c, u, message: (
        found.__setitem__(message.topic, message.payload) if message.retain else None
    )
    result, _ = client.subscribe([(topic, 1) for topic in topics])
    if result != mqtt.MQTT_ERR_SUCCESS:
        raise RuntimeError(f"could not subscribe to {topics}: error {result}")
    if not acknowledged.wait(ACK_SECONDS):
        raise TimeoutError(
            f"the broker didn't acknowledge the subscription to {topics}"
        )
    if refused:
        raise RuntimeError(
            f"the broker refused the subscription to {topics}: {refused}"
        )
    time.sleep(READ_SECONDS)
    client.unsubscribe(topics)
    client.on_subscribe = client.on_message = None
    return found


def publish(client, topic, payload, apply):
    action = "clear" if not payload else f"set {payload.decode(errors='replace')!r}"
    print(f"{'' if apply else '[dry run] '}{action}: {topic}")
    if apply:
        result = client.publish(topic, payload, qos=1, retain=True)
        result.wait_for_publish(timeout=10)
        if not result.is_published():
            raise TimeoutError(f"broker did not acknowledge {topic}")


def prepare(client, args):
    """Keep the light state and free the Home Assistant entity IDs, leaving the rest."""
    old, new = f"esp32_led_mqtt_{args.suffix}", f"esp32-led-{args.suffix}"
    retained = read_retained(client, [f"{old}/{topic}" for topic in STATE_TOPICS])
    for topic in STATE_TOPICS:
        payload = retained.get(f"{old}/{topic}")
        if payload is None:
            print(f"no retained {old}/{topic}; nothing to copy")
        else:
            publish(client, f"{new}/{topic}", payload, args.apply)
    # Copy first: discovery is what's lost if this is interrupted, and the old image
    # republishes it whenever it reconnects.
    for topic in old_discovery(args.suffix):
        publish(client, topic, b"", args.apply)


def finish(client, args):
    """Remove every old retained topic, once the new image is known to be running."""
    old, new = f"esp32_led_mqtt_{args.suffix}", f"esp32-led-{args.suffix}"
    status = read_retained(client, [f"{new}/status"]).get(f"{new}/status")
    if status != b"online":
        raise RuntimeError(
            f"{new}/status is {status!r}, not online; keeping old topics"
        )
    topics = [f"{old}/{topic}" for topic in STATE_TOPICS + OTHER_TOPICS]
    # An old image that reconnected since `prepare` has republished its discovery.
    for topic in topics + list(old_discovery(args.suffix)):
        publish(client, topic, b"", args.apply)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("step", choices=("prepare", "finish"))
    parser.add_argument("suffix", help="last six hex digits of the MAC address")
    parser.add_argument("--broker", required=True)
    parser.add_argument("--port", type=int, default=1883)
    parser.add_argument("--username")
    parser.add_argument("--tls", action="store_true")
    parser.add_argument("--apply", action="store_true", help="change the broker")
    args = parser.parse_args()
    if not re.fullmatch("[0-9a-f]{6}", args.suffix):
        parser.error("suffix must be six lowercase hex digits")
    client = connect(args)
    try:
        {"prepare": prepare, "finish": finish}[args.step](client, args)
    finally:
        client.loop_stop()
        client.disconnect()


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, TimeoutError) as error:
        print(f"Error: {error}", file=sys.stderr)
        sys.exit(1)

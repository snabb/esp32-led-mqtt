# CoAP reference

The `coap` feature adds a direct, authenticated API beside MQTT: everything MQTT offers,
plus settings, client keys, firmware upload and device logs. Home Assistant keeps using
MQTT. The feature is on by default, and the server starts once the device has a client key.

## Security

Every resource needs OSCORE (RFC 8613) keyed by EDHOC (RFC 9528), except discovery at
`/.well-known/core`, which lists only what the caller may access. Without a trusted key,
requests get 4.01.

A trusted client has full control of the device, including installing any firmware and
reading its logs. Images aren't signed, and `OTA_ALLOWED_HOSTS` doesn't apply to uploads.
Protect client keys accordingly.

### Keys

| Key | Lives in | Secret |
|---|---|---|
| Device private | The device's settings, generated there | Yes |
| Device public | `local/coap/esp32-led-XXXXXX.ccs` | No |
| Client private | `local/coap/client.key` | Yes |
| Client public | The device's settings, up to four | No |

No keys are compiled into firmware. Sessions live in RAM, so a client runs EDHOC again
after a reboot.

### First setup

This needs one USB connection for each device. Other client keys can then be added over
CoAP.

```sh
uv run tools/coap.py keygen
uv run tools/provision.py provision --port $PORT --config local/device.json \
  --coap-client local/coap/client.ccs
```

The device generates its key and replies with its public part, which `provision.py` saves
as `local/coap/esp32-led-XXXXXX.ccs`. Reprovisioning keeps the keys unless
`--coap-client` is given again, which replaces the client list. `erase-config` removes
them all, including the device's key.

### Client keys over CoAP

```sh
uv run tools/coap.py --host esp32-led-XXXXXX.local clients list
uv run tools/coap.py --host esp32-led-XXXXXX.local clients add other.ccs
uv run tools/coap.py --host esp32-led-XXXXXX.local clients remove other.ccs
```

- The reply means the change was accepted. It's saved just after, which `coap.py` checks. If
  saving fails, the device reboots to whatever is stored.
- The last key can't be removed, because recovering from that needs USB.
- Two keys can't share a key ID, because a device couldn't tell them apart. `keygen` picks a
  random one.
- Removing a key stops new handshakes, while a session it already has lasts until the
  device reboots. So `clients remove` reboots the device unless given `--no-reboot`.

## Using it

```sh
HOST="--host esp32-led-XXXXXX.local"
uv run tools/coap.py $HOST info
uv run tools/coap.py $HOST light '{"state":"ON","effect":"Pacifica"}'
uv run tools/coap.py $HOST speed 20
uv run tools/coap.py $HOST logs -f
uv run tools/coap.py $HOST push firmware.bin
```

Run it with `--help` for the rest. With an IP address, also give `--device-ccs FILE`. Key
files are found by host name otherwise.

## Resources

| Resource | Methods | Observe | Payload |
|---|---|---|---|
| `/light` | GET, PUT | Yes | The [MQTT light command](MQTT.md#light-command), JSON. Fields left out keep their values |
| `/speed` | GET, PUT | Yes | Integer `1`–`128` as text |
| `/temperature` | GET | Yes | °C as text, one decimal |
| `/info` | GET | | JSON: name, build, hardware, uptime, free heap, IP address |
| `/effects` | GET | | JSON list of effect names, block-wise |
| `/logs` | GET | Yes | See [logs](#logs) |
| `/config` | GET, PUT | | See [settings](#settings) |
| `/coap/clients` | GET, POST, DELETE | | See [keys](#client-keys-over-coap). GET gives fingerprints as JSON, POST takes a hex credential, DELETE takes `?fp=` |
| `/firmware` | GET | Yes | [OTA status](OTA.md#status-messages), JSON |
| `/firmware/update` | POST | | The [HTTPS OTA command](OTA.md#send-an-update), JSON |
| `/firmware/manifest` | PUT | | See [upload](#firmware-upload) |
| `/firmware/image` | PUT | | Block1 |
| `/reboot` | POST | | Empty |

A refused request's payload gives the reason as text, such as `busy`, `already_running`
or `last_key`.

## Settings

`GET /config` shows the settings in use, with only whether each password is set, and the
device's public credential.

`PUT /config` takes the same JSON as [USB provisioning](CONFIGURATION.md#device-settings),
as a whole, saves it and reboots. It refuses `coap_*` fields, and the stored keys are kept.

There's no rollback for settings. **Wrong Wi-Fi settings leave the device unreachable until
it's provisioned over USB.**

## Logs

CoAP builds keep the last 8 KiB of log output in RAM. It's lost on reboot, and a panic's
output only goes to USB.

Each reply starts with a line of two byte offsets, `start next`, followed by that range of
log text. Offsets count every byte since boot.

- `GET /logs?from=N` reads from offset `N`, or from the oldest text kept if that's gone.
  Keep asking with `from=next` until a reply has no text.
- `GET /logs` reads the newest text that fits.
- Observing `/logs` tails it, with at most one notification a second. Each carries the
  newest text, so a client that sees `start` beyond what it has read can fill the gap with
  `GET`. `logs -f` does this.

A `next` lower than the client's own offset means the device restarted.

## Firmware upload

`tools/coap.py push` does this for you:

1. `PUT /firmware/manifest` with a [HTTPS OTA command](OTA.md#send-an-update) without its
   `url`.
2. `PUT /firmware/image` with the image, using Block1. The manifest applies to one upload.
3. The final 2.04 means it's verified and activated. The device then reboots on
   [trial](OTA.md#trial-boot-and-rollback).
4. Any authenticated request counts as the control plane's part of the trial, so `push` polls
   `/firmware` until the trial has passed, or reports that it rolled back.

It needs an image built with `OTA_ROLLBACK_BOOTLOADER=1`, but not `OTA_ALLOWED_HOSTS`.
Starting an upload aborts one already in progress. One that stalls for 30 seconds is
abandoned, and the running image is never touched.

## Finding devices

With the `mdns` feature, a device answers as `esp32-led-XXXXXX.local`, and advertises
`_coap._udp` once it has CoAP keys:

```sh
avahi-browse -rt _coap._udp
```

A client on another subnet depends on the router repeating mDNS, which may be unreliable.
Use `--host IP --device-ccs FILE` if a name doesn't resolve.

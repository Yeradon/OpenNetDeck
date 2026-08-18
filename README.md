# OpenNetDeck

OpenNetDeck is an open-source, hardware-agnostic reimplementation of the Elgato Network Dock functionality in Rust.

It enables generic Linux machines, single-board computers, and embedded devices to act as a network bridge over TCP using Elgato's CORA protocol. This allows physical USB Stream Decks to be accessed over the network by both the official Elgato Stream Deck desktop application and Bitfocus Companion without requiring official network dock hardware.

> **Disclaimer:** This project is a proof of concept (PoC) and largely vibecoded through reverse-engineering. It is intended for experimentation, research, and self-hosted setups.

---

## Workspace Architecture

* **`crates/opennetdeck-protocol`:** Core `no_std` protocol definitions, CORA framing, keepalive state machines, and report serialization.
* **`crates/opennetdeck-server`:** Async Tokio daemon for Linux and macOS. Handles USB device discovery via `nusb`, mDNS discovery, and bridges physical devices to TCP endpoints.
* **`crates/opennetdeck-embedded`:** Bare-metal `no_std` runtime designed for microcontrollers using `embedded-io-async`. *(Status: Experimental / Untested on real hardware)*.

---

## Quick Start (Linux Daemon)

### Using Nix (Recommended)

```bash
# Run with default settings (Dock hub on 5343, child bridge on 5344)
nix run . -- --port 5343 --secondary-port 5344

# Or enter the dev shell
nix develop
cargo run -p opennetdeck-server -- --port 5343 --secondary-port 5344
```

### Using Cargo Directly

Make sure `libusb` and `udev` development headers are installed:

```bash
cargo run -p opennetdeck-server -- --port 5343 --secondary-port 5344
```

---

## Configuration & CLI Options

```text
Usage: opennetdeck-server [OPTIONS]

Options:
  -b, --bind <BIND>                      Bind IP address [default: 0.0.0.0]
  -p, --port <PORT>                      Primary dock TCP control port [default: 5343]
      --secondary-port <SECONDARY_PORT>  Base TCP port for child Stream Deck bridge [default: 5344]
      --mode <MODE>                      Operation mode: 'dock' or 'direct' [default: dock]
      --child-pid <CHILD_PID>            Override child Product ID (e.g. 0x0084 for Plus, 0x0080 for MK.2)
      --serial <SERIAL>                  Serial number reported for dock [default: DL01A1A00001]
      --firmware <FIRMWARE>              Firmware version string [default: 1.0.0.0]
      --mac <MAC>                        MAC address in hex format [default: 00:1a:7d:da:71:01]
      --no-mdns                          Disable mDNS / Bonjour advertisement
  -h, --help                             Print help
  -V, --version                          Print version
```

---

## Connecting Clients

### Official Elgato Stream Deck App
1. Run `opennetdeck-server` in default `dock` mode on the same local network.
2. Launch the Elgato Stream Deck app on macOS or Windows.
3. The app discovers the dock automatically via mDNS (`_elg._tcp.local.`), queries device info on port 5343, and attaches to the secondary bridge on port 5344.

### Bitfocus Companion
* **Dock Mode:** In Companion, add a surface under **Surfaces** -> **Add Network Surface** -> **Elgato Stream Deck (TCP)** targeting `<HOST_IP>:5344`.
* **Direct Mode:** Launch the server with `--mode direct --port 5343` and configure Companion to target `<HOST_IP>:5343`.

---

## Development

```bash
# Run test suite
cargo test --all-targets

# Linter and formatting checks
cargo fmt --check
cargo clippy --all-targets

# Flake build validation
nix flake check
```

---

## License

Dual-licensed under either of:

* Apache License, Version 2.0 (http://www.apache.org/licenses/LICENSE-2.0)
* MIT license (http://opensource.org/licenses/MIT)

at your option.

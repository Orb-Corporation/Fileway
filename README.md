# Fileway

**Fileway** is a free, direct file-transfer app for devices on the same local network. It is being developed by Orb Technologies.

> The project is under active development. Do not treat early builds as production-ready.

## Platforms

- Windows
- macOS
- Linux
- Android
- iOS is intentionally excluded.

## Transfer flow

1. Fileway announces its presence on the local network and discovers other Fileway devices.
2. A user sends a pairing request to a discovered device.
3. The receiving user explicitly accepts or rejects the pairing request.
4. A paired user selects files and sends a transfer request with file names and sizes.
5. The receiving user reviews the list and explicitly accepts or rejects the transfer.
6. Accepted files are streamed to the receiving device. The app does not load an entire file into memory.

## Technology

- Tauri 2 for desktop and Android packaging.
- TypeScript + Vite for the interface.
- Rust + Tokio for local-network discovery, pairing, and streaming transfer.
- UDP broadcast for LAN discovery; TCP for pairing and file data.

## Development setup

Install Node.js LTS, Rust, and the prerequisites for Tauri 2 on your operating system.

    npm install
    npm run tauri dev

Build the frontend:

    npm run build

Build a desktop package:

    npm run tauri build

For Android, initialize the Tauri Android project once, apply the manifest permissions, then use the Android toolchain:

    npm run tauri:android:init
    npm run tauri android dev

The Android initializer adds the network permissions to the generated manifest. Android 17 (API 37) requires apps targeting API 37 or higher to request local-network access at runtime; the native runtime permission prompt is not implemented yet, so Android 17+ targeting/API policy compatibility remains a release blocker. macOS asks the user to grant local-network access on first use.

## Network and security notes

- Discovery uses UDP broadcast on port 39147; transfers use TCP on port 39148.
- Both devices must be on the same reachable LAN. Guest Wi-Fi isolation, VLAN separation, VPNs, or firewall rules can prevent discovery or transfer.
- Pairing approval and per-transfer approval are separate. A discovered device cannot upload files merely by appearing in the list.
- **Initial development builds do not yet provide end-to-end encryption.** Use only on a trusted private network and do not transfer sensitive files until authenticated encryption and security review are complete.
- Incoming files are written under the app's private Received data directory. Export-to-folder and Android public-storage handling are still being implemented.

## License

To be selected by the project owner before public distribution.

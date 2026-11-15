# Fileway manual test plan

This document is the acceptance checklist for testing Fileway on real devices. CI compilation is not a substitute for two-device LAN testing.

## Test environment

Use two devices with Fileway installed and open. Connect them to the same reachable Wi-Fi or Ethernet network. For desktop testing, permit Fileway through the operating system firewall when prompted.

The current network protocol uses UDP broadcast on port **39147** for discovery and TCP on port **39148** for pairing and file data. Guest networks with client isolation, separate VLANs, VPN routes, and restrictive firewalls may block traffic even when both devices have internet access.

## Pairing

- [ ] Both devices discover one another within 12 seconds.
- [ ] The device name and local IP address are displayed on the receiving side.
- [ ] A pairing request appears only on the requested receiver.
- [ ] Rejecting the request leaves both devices unpaired.
- [ ] Accepting the request stores the peer on both devices.
- [ ] Restart both apps and confirm the paired relationship remains.
- [ ] Remove the pairing on one device and verify that it cannot start a new transfer with that peer.
- [ ] A device that is not paired cannot submit an accepted transfer request.

## File transfer

Test one file first, then a batch.

- [ ] Receiver sees every file name and byte size before accepting.
- [ ] Rejecting a transfer sends no file data.
- [ ] Accepting a transfer writes the files into the receiver's private Received directory.
- [ ] Sender and receiver progress indicators advance and reach the completed state.
- [ ] Zero-byte files are transferred correctly.
- [ ] Test ordinary files, large files, Unicode names, spaces, and duplicate names.
- [ ] Existing files are not silently overwritten; a numbered name is chosen instead.
- [ ] Exporting a received file through the Save dialog produces a file with the expected size and contents.
- [ ] Multi-file transfers preserve file boundaries and order.

## Interruption and invalid input

- [ ] Close the sender during an active transfer. The receiver must remove the incomplete part file.
- [ ] Stop the receiver during an active transfer. The sender must report a failure, not success.
- [ ] Leave an accepted transfer idle for more than 90 seconds. The receiver must time out and clean up the partial file.
- [ ] Send a file whose name contains path separators or platform-invalid characters. It must be stored as a safe leaf filename.
- [ ] Try to send more than 128 files or a batch larger than 100 GB. The receiver must reject the request.
- [ ] Confirm the pending-request UI clears after rejection, timeout, or failed disk write.

## Cross-platform matrix

Use actual hardware for Android and macOS permission tests.

| Sender | Receiver | Status |
|---|---|---|
| Windows | Windows | Not physically tested |
| Windows | Linux | Not physically tested |
| Windows | macOS | Not physically tested |
| Windows | Android | Not physically tested |
| Linux | Android | Not physically tested |
| macOS | Android | Not physically tested |
| Android | Android | Not physically tested |

Add more combinations when a platform-specific bug is found.

## Release blockers

- Network frames and pairing tokens are currently unencrypted. Do not transfer sensitive files or use untrusted networks until authenticated encryption, key management, and a security review are complete.
- Android is deliberately kept at compileSdk/targetSdk 36. Do not raise targetSdk to 37 until the Android 17 ACCESS_LOCAL_NETWORK runtime-permission flow is implemented.
- Android file picker and Save dialog results must be tested with local storage and cloud-backed document providers.
- Test firewall behavior and discovery on home, school/guest, and enterprise networks. Do not assume all Wi-Fi networks allow device-to-device traffic.

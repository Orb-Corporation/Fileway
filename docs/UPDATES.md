# Fileway updates

Desktop in-app updates use Tauri's signed updater. Update packages are signature-checked before installation.

## One-time signing-key setup

From the repository root, run:

```powershell
npm install
npm run tauri signer generate -- -w "$HOME/.tauri/fileway.key"
```

The command creates a private key and a public key file. Keep the private key backed up. Never commit or share it.

1. Copy the complete contents of `fileway.key.pub` into `plugins.updater.pubkey` in `src-tauri/tauri.conf.json`, replacing `REPLACE_WITH_FILEWAY_TAURI_PUBLIC_KEY`.
2. In GitHub, open **Orb-Corporation / Fileway → Settings → Secrets and variables → Actions**.
3. Add `TAURI_SIGNING_PRIVATE_KEY` with the complete contents of `fileway.key`.
4. Add `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` with the key password. Use an empty value only if the key was generated without a password.
5. Keep an offline backup of the private key and its password. Losing the private key prevents future signed updates from being accepted by existing installs.

## Publish a release

Increase the same semantic version in `package.json` and `src-tauri/tauri.conf.json`. Commit the change, then push a version tag:

```bash
git tag v0.2.0
git push origin v0.2.0
```

The **Fileway release** workflow builds desktop packages, signs the updater artifacts, and attaches the update manifest to the GitHub Release. Installed desktop copies can then use **Check for updates**.

Android remains distributed as a signed APK through its distribution channel. An APK can replace an existing installation only when the application ID and signing identity match. iOS is not currently included in Fileway's supported builds.

import { readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";

const manifestPath = resolve(
  "src-tauri/gen/android/app/src/main/AndroidManifest.xml",
);

let manifest;
try {
  manifest = readFileSync(manifestPath, "utf8");
} catch {
  console.error("Android manifest not found. Run `npx tauri android init` first.");
  process.exit(1);
}

const permissions = [
  "android.permission.INTERNET",
  "android.permission.ACCESS_NETWORK_STATE",
  "android.permission.ACCESS_LOCAL_NETWORK",
];

for (const permission of permissions) {
  const declaration = `<uses-permission android:name="${permission}" />`;
  if (manifest.includes(`android:name="${permission}"`)) continue;
  const applicationTag = manifest.indexOf("<application");
  if (applicationTag < 0) {
    throw new Error("AndroidManifest.xml has no <application> element.");
  }
  manifest = manifest.slice(0, applicationTag)
    + `    ${declaration}\\n    `
    + manifest.slice(applicationTag);
}

writeFileSync(manifestPath, manifest, "utf8");
console.log("Fileway Android network permissions added.");

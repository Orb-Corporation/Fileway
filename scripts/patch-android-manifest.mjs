import { readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";

const manifestPath = resolve(
  "src-tauri/gen/android/app/src/main/AndroidManifest.xml",
);
const gradlePath = resolve(
  "src-tauri/gen/android/app/build.gradle.kts",
);

let manifest;
let gradle;
try {
  manifest = readFileSync(manifestPath, "utf8");
  gradle = readFileSync(gradlePath, "utf8");
} catch {
  console.error("Android project not found. Run `npx tauri android init` first.");
  process.exit(1);
}

// Tauri 2.12 currently generates targetSdk 37. Fileway deliberately stays at
// API 36 until its Android 17 ACCESS_LOCAL_NETWORK runtime permission bridge
// is implemented. Android 17 preserves implicit LAN access for targetSdk <= 36.
const targetSdkPattern = /(targetSdk(?:Version)?\s*(?:=\s*|\s+)?)\d+/g;
let targetSdkMatches = 0;
gradle = gradle.replace(targetSdkPattern, (match, prefix) => {
  targetSdkMatches += 1;
  return `${prefix}36`;
});
if (targetSdkMatches === 0) {
  throw new Error(
    "Could not identify targetSdk in app/build.gradle.kts. Review the generated Gradle file before building.",
  );
}

const permissions = [
  "android.permission.INTERNET",
  "android.permission.ACCESS_NETWORK_STATE",
];

for (const permission of permissions) {
  const declaration = `<uses-permission android:name="${permission}" />`;
  if (manifest.includes(`android:name="${permission}"`)) continue;
  const applicationTag = manifest.indexOf("<application");
  if (applicationTag < 0) {
    throw new Error("AndroidManifest.xml has no <application> element.");
  }
  manifest =
    manifest.slice(0, applicationTag) +
    `    ${declaration}\n    ` +
    manifest.slice(applicationTag);
}

// Do not declare ACCESS_LOCAL_NETWORK while targeting API 36. Android 17
// only requires and enforces that permission for apps targeting API 37+.
manifest = manifest.replace(
  /\s*<uses-permission\s+android:name="android\.permission\.ACCESS_LOCAL_NETWORK"\s*\/>\s*/g,
  "\n",
);

writeFileSync(manifestPath, manifest, "utf8");
writeFileSync(gradlePath, gradle, "utf8");
console.log("Fileway Android setup complete (targetSdk 36, LAN permissions configured).");

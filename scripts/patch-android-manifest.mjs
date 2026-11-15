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

// Tauri 2.12 templates currently use API 37. Fileway stays on API 36 until
// the native Android 17 ACCESS_LOCAL_NETWORK runtime-permission flow exists.
// Android 17 preserves implicit local-network access for targetSdk <= 36.
const sdkFields = [
  { name: "compileSdk", pattern: /(compileSdk(?:Version)?\s*(?:=\s*|\s+)?)\d+/g },
  { name: "targetSdk", pattern: /(targetSdk(?:Version)?\s*(?:=\s*|\s+)?)\d+/g },
];

for (const field of sdkFields) {
  let matches = 0;
  gradle = gradle.replace(field.pattern, (_match, prefix) => {
    matches += 1;
    return `${prefix}36`;
  });
  if (matches === 0) {
    throw new Error(
      `Could not identify ${field.name} in app/build.gradle.kts. Review the generated Gradle file before building.`,
    );
  }
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

// ACCESS_LOCAL_NETWORK is only required when targeting API 37 or later.
manifest = manifest.replace(
  /\s*<uses-permission\s+android:name="android\.permission\.ACCESS_LOCAL_NETWORK"\s*\/>\s*/g,
  "\n",
);

writeFileSync(manifestPath, manifest, "utf8");
writeFileSync(gradlePath, gradle, "utf8");
console.log("Fileway Android setup complete (compileSdk 36, targetSdk 36).");

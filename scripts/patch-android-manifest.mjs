import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";

const manifestPath = resolve(
  "src-tauri/gen/android/app/src/main/AndroidManifest.xml",
);
const gradlePath = resolve(
  "src-tauri/gen/android/app/build.gradle.kts",
);
const keystorePropertiesPath = resolve(
  "src-tauri/gen/android/keystore.properties",
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

// Configure release signing only in CI jobs where the private keystore has
// been provisioned. Local debug builds remain usable without signing secrets.
if (existsSync(keystorePropertiesPath)) {
  if (!/^import java\.util\.Properties$/m.test(gradle)) {
    gradle = `import java.util.Properties\n${gradle}`;
  }

  if (!gradle.includes('create("release")')) {
    const buildTypesMatch = gradle.match(/^([ \t]*)buildTypes[ \t]*\{/m);
    if (!buildTypesMatch || buildTypesMatch.index === undefined) {
      throw new Error(
        "Could not locate buildTypes in app/build.gradle.kts; refusing to patch signing blindly.",
      );
    }

    const indent = buildTypesMatch[1];
    const signingBlock = [
      `${indent}signingConfigs {`,
      `${indent}    create("release") {`,
      `${indent}        val keystorePropertiesFile = rootProject.file("keystore.properties")`,
      `${indent}        if (!keystorePropertiesFile.exists()) {`,
      `${indent}            throw GradleException("Missing Android release keystore.properties")`,
      `${indent}        }`,
      `${indent}        val keystoreProperties = Properties()`,
      `${indent}        keystorePropertiesFile.inputStream().use { keystoreProperties.load(it) }`,
      `${indent}        keyAlias = keystoreProperties["keyAlias"] as String`,
      `${indent}        keyPassword = keystoreProperties["password"] as String`,
      `${indent}        storeFile = file(keystoreProperties["storeFile"] as String)`,
      `${indent}        storePassword = keystoreProperties["password"] as String`,
      `${indent}    }`,
      `${indent}}`,
      "",
    ].join("\n");

    const insertAt = buildTypesMatch.index;
    gradle =
      gradle.slice(0, insertAt) +
      signingBlock +
      gradle.slice(insertAt);
  }

  const releaseBuildTypeMatch = gradle.match(/^([ \t]*)getByName\("release"\)[ \t]*\{/m);
  if (!releaseBuildTypeMatch || releaseBuildTypeMatch.index === undefined) {
    throw new Error(
      "Could not locate the release build type in app/build.gradle.kts; refusing to create an unsigned release APK.",
    );
  }

  if (!/signingConfig\s*=\s*signingConfigs\.getByName\("release"\)/.test(gradle)) {
    const insertAt =
      releaseBuildTypeMatch.index + releaseBuildTypeMatch[0].length;
    const statement = `\n${releaseBuildTypeMatch[1]}    signingConfig = signingConfigs.getByName("release")`;
    gradle =
      gradle.slice(0, insertAt) +
      statement +
      gradle.slice(insertAt);
  }

  console.log("Android release signing configured from keystore.properties.");
}

writeFileSync(manifestPath, manifest, "utf8");
writeFileSync(gradlePath, gradle, "utf8");
console.log("Fileway Android setup complete (compileSdk 36, targetSdk 36).");

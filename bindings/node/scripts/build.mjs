import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const pkgRoot = path.resolve(__dirname, "..");
const repoRoot = path.resolve(pkgRoot, "..", "..");
const includeDir = path.join(repoRoot, "include");
const srcFile = path.join(pkgRoot, "src", "oryq_napi.c");
const buildDir = path.join(pkgRoot, "build");
const outFile = path.join(buildDir, "oryq_napi.node");

function findNodeInclude() {
  const candidates = [
    path.resolve(path.dirname(process.execPath), "..", "include", "node"),
    "/usr/include/node",
    "/usr/local/include/node",
  ];
  for (const dir of candidates) {
    if (fs.existsSync(path.join(dir, "node_api.h"))) {
      return dir;
    }
  }
  throw new Error(
    `Could not locate node_api.h (searched: ${candidates.join(", ")})`,
  );
}

function findLibDir() {
  if (process.env.ORYQ_LIB_DIR) {
    return process.env.ORYQ_LIB_DIR;
  }
  const libName =
    process.platform === "darwin" ? "liboryq.dylib" : "liboryq.so";
  const candidates = ["release", "debug"]
    .map((p) => path.join(repoRoot, "target", p, libName))
    .filter((f) => fs.existsSync(f))
    .sort((a, b) => fs.statSync(b).mtimeMs - fs.statSync(a).mtimeMs);
  if (candidates.length === 0) {
    throw new Error(
      `Could not locate ${libName} under target/{release,debug}. Run \`cargo build -p oryq-ffi\` first.`,
    );
  }
  return path.dirname(candidates[0]);
}

if (process.platform === "win32") {
  throw new Error(
    "Zero-dependency build.mjs currently targets Linux and macOS (cc). On Windows, compile src/oryq_napi.c against node.lib and oryq.dll.lib.",
  );
}

fs.mkdirSync(buildDir, { recursive: true });
const nodeInc = findNodeInclude();
const libDir = findLibDir();
const cc = process.env.CC || "cc";

const args = [
  "-std=c99",
  "-O2",
  "-Wall",
  "-Wextra",
  "-Werror",
  "-fPIC",
  `-I${nodeInc}`,
  `-I${includeDir}`,
  srcFile,
  `-L${libDir}`,
  "-loryq",
  `-Wl,-rpath,${libDir}`,
  "-lm",
];

if (process.platform === "darwin") {
  args.push("-bundle", "-undefined", "dynamic_lookup");
} else {
  args.push("-shared");
}
args.push("-o", outFile);

execFileSync(cc, args, { stdio: "inherit" });
console.log(`Built ${outFile} (linked against ${libDir})`);

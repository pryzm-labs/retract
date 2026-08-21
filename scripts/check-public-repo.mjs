import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, extname, resolve } from "node:path";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const errors = [];
const requiredFiles = [
  "LICENSE",
  "SECURITY.md",
  "CONTRIBUTING.md",
  "CODE_OF_CONDUCT.md",
  "CHANGELOG.md",
  "THIRD_PARTY_NOTICES.md",
  ".github/ISSUE_TEMPLATE/bug_report.yml",
  ".github/ISSUE_TEMPLATE/feature_request.yml",
  ".github/ISSUE_TEMPLATE/config.yml",
  ".github/PULL_REQUEST_TEMPLATE.md",
  ".github/dependabot.yml",
  "docs/images/retract-overview.png"
];

for (const path of requiredFiles) {
  if (!existsSync(resolve(root, path))) errors.push(`missing required public file: ${path}`);
}

function cargoVersion(path) {
  const text = readFileSync(resolve(root, path), "utf8");
  const section = text.match(/\[package\]([\s\S]*?)(?:\n\[|$)/)?.[1] || "";
  return {
    version: section.match(/^version\s*=\s*"([^"]+)"/m)?.[1],
    license: section.match(/^license\s*=\s*"([^"]+)"/m)?.[1]
  };
}

const packageJson = JSON.parse(readFileSync(resolve(root, "package.json"), "utf8"));
const tauri = JSON.parse(readFileSync(resolve(root, "src-tauri/tauri.conf.json"), "utf8"));
const applicationCargo = cargoVersion("src-tauri/Cargo.toml");
const domainCargo = cargoVersion("crates/cleaner-domain/Cargo.toml");
const expectedNode = readFileSync(resolve(root, ".nvmrc"), "utf8").trim();

if (packageJson.license !== "MIT") errors.push("package.json license must be exactly MIT");
if (applicationCargo.license !== "MIT") errors.push("src-tauri Cargo license must be exactly MIT");
if (domainCargo.license !== "MIT") errors.push("domain Cargo license must be exactly MIT");
if (packageJson.engines?.node !== expectedNode) errors.push(`package.json engines.node must equal ${expectedNode}`);
if (new Set([packageJson.version, tauri.version, applicationCargo.version, domainCargo.version]).size !== 1) {
  errors.push("package, Tauri, and Cargo versions must match");
}

const readme = readFileSync(resolve(root, "README.md"), "utf8");
for (const reference of ["assets/retract-icon.png", "docs/images/retract-overview.png"]) {
  if (!readme.includes(reference)) errors.push(`README must reference ${reference}`);
}

const privacyInstruction = /do not (?:attach|upload|include)[^\n]*(?:chat exports|credentials|session files|unredacted conversations)/i;
for (const form of ["bug_report.yml", "feature_request.yml"]) {
  const path = resolve(root, ".github/ISSUE_TEMPLATE", form);
  if (existsSync(path) && !privacyInstruction.test(readFileSync(path, "utf8"))) {
    errors.push(`${form} must warn against uploading private Telegram material`);
  }
}

function markdownFiles(directory) {
  const results = [];
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    if (entry.name === ".git" || entry.name === "node_modules" || entry.name === "target" || entry.name === "tdlib-source") continue;
    const path = resolve(directory, entry.name);
    if (entry.isDirectory()) results.push(...markdownFiles(path));
    else if (extname(entry.name) === ".md") results.push(path);
  }
  return results;
}

for (const markdown of markdownFiles(root)) {
  const text = readFileSync(markdown, "utf8");
  for (const match of text.matchAll(/!?\[[^\]]*\]\(([^)]+)\)/g)) {
    let destination = match[1].trim().replace(/^<|>$/g, "").split(/\s+['"]/)[0];
    if (!destination || destination.startsWith("#") || /^[a-z][a-z0-9+.-]*:/i.test(destination)) continue;
    destination = decodeURIComponent(destination.split("#")[0]);
    const resolved = resolve(dirname(markdown), destination);
    if (!existsSync(resolved)) {
      errors.push(`broken relative Markdown link in ${markdown.slice(root.length + 1)}: ${destination}`);
    } else {
      statSync(resolved);
    }
  }
}

const workflowsDirectory = resolve(root, ".github/workflows");
const workflowPaths = existsSync(workflowsDirectory)
  ? readdirSync(workflowsDirectory)
      .filter((name) => /\.ya?ml$/i.test(name))
      .map((name) => resolve(workflowsDirectory, name))
  : [];

for (const workflowPath of workflowPaths) {
  const workflow = readFileSync(workflowPath, "utf8");
  for (const match of workflow.matchAll(/^\s*uses:\s*([^\s#]+)/gm)) {
    const reference = match[1];
    if (!reference.startsWith("./") && !/@[0-9a-f]{40}$/i.test(reference)) {
      errors.push(`workflow action is not pinned to a full commit SHA in ${workflowPath.slice(root.length + 1)}: ${reference}`);
    }
  }
}

const releaseWorkflowPath = resolve(workflowsDirectory, "release.yml");
if (!existsSync(releaseWorkflowPath)) {
  errors.push("missing tag release workflow: .github/workflows/release.yml");
} else {
  const releaseWorkflow = readFileSync(releaseWorkflowPath, "utf8");
  const topLevel = releaseWorkflow.split(/^jobs:\s*$/m)[0];
  const releaseJob = releaseWorkflow.match(/^  release-preview:\s*$([\s\S]*)/m)?.[1] || "";

  if (!/^\s{2}push:\s*\n\s{4}tags:\s*\n\s{6}-\s*["']v\*["']\s*$/m.test(releaseWorkflow)) {
    errors.push("release workflow must trigger on v* tags");
  }
  if (!/^\s{2}workflow_dispatch:\s*$/m.test(releaseWorkflow)) {
    errors.push("release workflow must support manual tag dispatch");
  }
  if (/^\s{2}(?:pull_request|schedule):/m.test(releaseWorkflow) || /^\s{4}branches:/m.test(releaseWorkflow)) {
    errors.push("release workflow must not run for branches, pull requests, or schedules");
  }
  if (!/^permissions:\s*\n\s{2}contents:\s*read\s*$/m.test(topLevel) || /:\s*write\s*$/m.test(topLevel)) {
    errors.push("release workflow must be read-only by default");
  }
  for (const permission of ["contents", "id-token", "attestations", "artifact-metadata"]) {
    if (!new RegExp(`^\\s{6}${permission}:\\s*write\\s*$`, "m").test(releaseJob)) {
      errors.push(`release-preview job must grant ${permission}: write`);
    }
    if ((releaseWorkflow.match(new RegExp(`^\\s+${permission}:\\s*write\\s*$`, "gm")) || []).length !== 1) {
      errors.push(`${permission}: write must appear only on the release-preview job`);
    }
  }
  if (!/uses:\s*actions\/attest@[0-9a-f]{40}/i.test(releaseJob)) {
    errors.push("release-preview job must attest release provenance");
  }
  if (!/persist-credentials:\s*false/.test(releaseWorkflow)) {
    errors.push("release checkouts must not persist GitHub credentials");
  }
  if (!/npm run package:unsigned/.test(releaseWorkflow)) {
    errors.push("release workflow must call npm run package:unsigned");
  }
  for (const suffix of [".app.zip", ".app.zip.sha256", ".app.zip.manifest.json"]) {
    if (!releaseJob.includes(`Retract-*${suffix}`)) {
      errors.push(`release-preview job must publish only the named ${suffix} artifact`);
    }
  }
  if (!/gh release create[\s\S]*--prerelease/.test(releaseJob)) {
    errors.push("release-preview job must create a GitHub pre-release");
  }
}

if (errors.length > 0) {
  console.error(errors.map((error) => `- ${error}`).join("\n"));
  process.exit(1);
}

console.log("Public repository metadata and documentation are consistent.");

#!/usr/bin/env node
// 校验 Tauri IPC 契约：
//   A. 每个 #[tauri::command] 定义都注册在 lib.rs 的 generate_handler! 中；
//   B. generate_handler! 的每个命令都能在源码中找到定义；
//   C. 前端 api.ts 中每个非 plugin 的 invoke 命令名都存在于注册集合中。
// 差异输出到 stderr 并以 exit 1 结束；全部一致时输出统计。

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const COMMANDS_DIR = path.join(ROOT, "src-tauri/src/commands");
const DEV_API_RS = path.join(ROOT, "src-tauri/src/dev_api.rs");
const LIB_RS = path.join(ROOT, "src-tauri/src/lib.rs");
const API_TS = path.join(ROOT, "src/lib/api.ts");

const rel = (file) => path.relative(ROOT, file);

function fatal(message) {
  console.error(`check:contracts: ${message}`);
  process.exit(1);
}

function lineOf(source, index) {
  let line = 1;
  for (let i = 0; i < index; i += 1) if (source[i] === "\n") line += 1;
  return line;
}

function listRustFiles(dir) {
  const files = [];
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) files.push(...listRustFiles(full));
    else if (entry.isFile() && entry.name.endsWith(".rs")) files.push(full);
  }
  return files.sort();
}

// `#[tauri::command...]` 与函数定义之间可能有文档注释或其他属性。
function extractCommandDefinitions(file) {
  const source = fs.readFileSync(file, "utf8");
  const lines = source.split("\n");
  const defs = [];
  for (let i = 0; i < lines.length; i += 1) {
    if (!/^\s*#\[tauri::command\b/.test(lines[i])) continue;
    let found = false;
    const limit = Math.min(lines.length, i + 32);
    for (let j = i + 1; j < limit; j += 1) {
      const line = lines[j];
      const trimmed = line.trim();
      if (trimmed === "" || trimmed.startsWith("//") || trimmed.startsWith("#[")) {
        continue;
      }
      const match = line.match(/\bfn\s+([A-Za-z_][A-Za-z0-9_]*)/);
      if (match) {
        defs.push({ name: match[1], file, line: j + 1 });
        found = true;
      }
      break;
    }
    if (!found) {
      fatal(
        `cannot find function after #[tauri::command] at ${rel(file)}:${i + 1}`,
      );
    }
  }
  return defs;
}

function parseHandlerRegistrations() {
  const source = fs.readFileSync(LIB_RS, "utf8");
  const marker = "generate_handler!";
  const markerIndex = source.indexOf(marker);
  if (markerIndex === -1) fatal(`no generate_handler! in ${rel(LIB_RS)}`);

  const open = source.indexOf("[", markerIndex);
  if (open === -1) fatal(`generate_handler! has no bracket in ${rel(LIB_RS)}`);

  let depth = 0;
  let close = -1;
  for (let i = open; i < source.length; i += 1) {
    if (source[i] === "[") depth += 1;
    else if (source[i] === "]") {
      depth -= 1;
      if (depth === 0) {
        close = i;
        break;
      }
    }
  }
  if (close === -1) fatal(`unterminated generate_handler! in ${rel(LIB_RS)}`);

  const body = source.slice(open + 1, close);
  const baseLine = lineOf(source, open);
  const entries = [];
  body.split("\n").forEach((rawLine, offset) => {
    const line = rawLine.replace(/\/\/.*$/, "").trim();
    if (line === "") return;
    for (const rawEntry of line.split(",")) {
      const entry = rawEntry.trim();
      if (entry === "") continue;
      if (!/^[A-Za-z_][A-Za-z0-9_]*(?:::[A-Za-z_][A-Za-z0-9_]*)+$/.test(entry)) {
        fatal(
          `cannot parse generate_handler! entry "${entry}" at ${rel(LIB_RS)}:${baseLine + offset}`,
        );
      }
      const segments = entry.split("::");
      entries.push({
        path: entry,
        module: segments.slice(0, -1).join("::"),
        name: segments.at(-1),
        line: baseLine + offset,
      });
    }
  });
  return entries;
}

function skipWhitespace(source, index) {
  let i = index;
  while (i < source.length && /\s/.test(source[i])) i += 1;
  return i;
}

function extractFrontendInvokes() {
  const source = fs.readFileSync(API_TS, "utf8");
  const invokes = [];
  const pattern = /\binvoke\b/g;
  let match;
  while ((match = pattern.exec(source)) !== null) {
    const start = match.index;
    const previous = source[start - 1] ?? "";
    if (/[\w$.]/.test(previous)) continue;

    const lineStart = source.lastIndexOf("\n", start - 1) + 1;
    if (source.slice(lineStart, start).includes("//")) continue;

    let i = skipWhitespace(source, start + match[0].length);

    // 跳过泛型参数，例如 invoke<TaskDetail[]>(。
    if (source[i] === "<") {
      let depth = 0;
      let closed = false;
      for (; i < source.length; i += 1) {
        if (source[i] === "<") depth += 1;
        else if (source[i] === ">" && source[i - 1] !== "=") {
          depth -= 1;
          if (depth === 0) {
            i += 1;
            closed = true;
            break;
          }
        }
      }
      if (!closed) fatal(`unterminated invoke generic at ${rel(API_TS)}:${lineOf(source, start)}`);
      i = skipWhitespace(source, i);
    }

    if (source[i] !== "(") continue;
    i = skipWhitespace(source, i + 1);

    const quote = source[i];
    const line = lineOf(source, start);
    if (quote !== '"' && quote !== "'" && quote !== "`") {
      invokes.push({ name: null, line });
      continue;
    }

    let value = "";
    let end = i + 1;
    let closed = false;
    while (end < source.length) {
      const char = source[end];
      if (char === "\\") {
        value += source[end + 1] ?? "";
        end += 2;
        continue;
      }
      if (char === quote) {
        closed = true;
        break;
      }
      value += char;
      end += 1;
    }
    if (!closed) fatal(`unterminated invoke command string at ${rel(API_TS)}:${line}`);
    invokes.push({ name: value, line });
    pattern.lastIndex = end + 1;
  }
  return invokes;
}

const commandFiles = listRustFiles(COMMANDS_DIR);
let definitions = commandFiles.flatMap(extractCommandDefinitions);
const devApiDefinitions = extractCommandDefinitions(DEV_API_RS);
const definitionsByModule = { commands: definitions.length, dev_api: devApiDefinitions.length };
definitions = [...definitions, ...devApiDefinitions];

const registrations = parseHandlerRegistrations();
const registeredNames = new Set(registrations.map((entry) => entry.name));
const definedNames = new Set(definitions.map((def) => def.name));

const invokes = extractFrontendInvokes();
const pluginInvokes = invokes.filter((call) => call.name !== null && call.name.startsWith("plugin:"));
const dynamicInvokes = invokes.filter((call) => call.name === null);
const frontendCommands = invokes.filter(
  (call) => call.name !== null && !call.name.startsWith("plugin:"),
);

const unregistered = definitions.filter((def) => !registeredNames.has(def.name));
const undefinedRegistrations = registrations.filter((entry) => !definedNames.has(entry.name));
const unknownFrontend = [
  ...new Map(
    frontendCommands
      .filter((call) => !registeredNames.has(call.name))
      .map((call) => [`${call.name}\u0000${call.line}`, call]),
  ).values(),
];

const problems = [];

if (unregistered.length > 0) {
  problems.push(
    `[A] #[tauri::command] defined but not registered in generate_handler! (${unregistered.length}):\n` +
      unregistered.map((def) => `  - ${def.name} (${rel(def.file)}:${def.line})`).join("\n"),
  );
}
if (undefinedRegistrations.length > 0) {
  problems.push(
    `[B] registered in generate_handler! but no #[tauri::command] definition (${undefinedRegistrations.length}):\n` +
      undefinedRegistrations
        .map((entry) => `  - ${entry.path} (${rel(LIB_RS)}:${entry.line})`)
        .join("\n"),
  );
}
if (dynamicInvokes.length > 0) {
  problems.push(
    `[C] invoke() command name is not a static string (${dynamicInvokes.length}):\n` +
      dynamicInvokes.map((call) => `  - ${rel(API_TS)}:${call.line}`).join("\n"),
  );
}
if (unknownFrontend.length > 0) {
  problems.push(
    `[C] frontend invoke() has no registered command (${unknownFrontend.length}):\n` +
      unknownFrontend
        .map((call) => `  - ${call.name} (${rel(API_TS)}:${call.line})`)
        .join("\n"),
  );
}

if (problems.length > 0) {
  console.error("check:contracts: FAILED\n");
  console.error(problems.join("\n\n"));
  console.error(
    `\ncheck:contracts: ${unregistered.length + undefinedRegistrations.length + unknownFrontend.length + dynamicInvokes.length} contract drift(s) detected.`,
  );
  process.exit(1);
}

const uniqueFrontend = new Set(frontendCommands.map((call) => call.name));
console.log("check:contracts: OK");
console.log(
  `  Rust definitions: ${definitions.length} (commands/: ${definitionsByModule.commands}, dev_api: ${definitionsByModule.dev_api})`,
);
console.log(`  generate_handler! registrations: ${registrations.length}`);
console.log(
  `  Frontend invoke(): ${invokes.length} call(s), ${uniqueFrontend.size} unique command(s), ${pluginInvokes.length} plugin call(s) skipped`,
);
console.log(`  Contract drift: 0`);

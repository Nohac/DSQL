import { existsSync, mkdirSync, readFileSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { Project, QuoteKind } from "@dsql/typescript/renderer";

export function createGeneratorProject(): Project {
  return new Project({
    manipulationSettings: {
      quoteKind: QuoteKind.Double,
    },
  });
}

export function createSourceFromTemplate(
  project: Project,
  outDir: string,
  name: string,
  generatorUrl: string,
) {
  mkdirSync(outDir, { recursive: true });
  return project.createSourceFile(
    join(outDir, name),
    templateContents(name, generatorUrl),
    { overwrite: true },
  );
}

function templateContents(name: string, generatorUrl: string): string {
  return readFileSync(templatePath(name, generatorUrl), "utf8").replace(
    /^\/\/ @ts-nocheck\r?\n/,
    "",
  );
}

function templatePath(name: string, generatorUrl: string): string {
  const generatorDir = dirname(fileURLToPath(generatorUrl));
  const candidates = [
    join(generatorDir, "templates", name),
    join(dirname(generatorDir), "templates", name),
  ];
  const path = candidates.find((candidate) => existsSync(candidate));
  if (!path) {
    throw new Error(`missing TanStack template ${name}`);
  }
  return path;
}

export function toPascalCase(value: string): string {
  const result = value
    .split(/[^A-Za-z0-9]+/)
    .filter(Boolean)
    .map((part) => `${part.charAt(0).toUpperCase()}${part.slice(1)}`)
    .join("");

  if (!result) {
    return "Operation";
  }

  return /^[0-9]/.test(result) ? `_${result}` : result;
}

export function importSpecifier(
  root: string,
  fromFile: string,
  modulePath: string,
): string {
  if (!modulePath.startsWith(".")) {
    return modulePath;
  }

  const absoluteModulePath = resolve(root, modulePath);
  const relativePath = relative(dirname(fromFile), absoluteModulePath)
    .split("\\")
    .join("/");
  return relativePath.startsWith(".") ? relativePath : `./${relativePath}`;
}

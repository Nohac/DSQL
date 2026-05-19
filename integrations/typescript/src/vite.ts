export type DsqlVitePluginOptions = {
  readonly generatedModule?: string;
};

export type VitePlugin = {
  readonly name: string;
  transform(code: string, id: string): TransformResult;
};

export type TransformResult =
  | string
  | {
      readonly code: string;
      readonly map?: null;
    }
  | null;

const DEFAULT_GENERATED_MODULE = "/src/generated/dsql/queries";
const SUPPORTED_EXTENSIONS = [".ts", ".tsx", ".js", ".jsx"];
const DSQL_TAG_PATTERN =
  /(export\s+)?(?:const|let|var)\s+([A-Za-z_$][A-Za-z0-9_$]*)\s*=\s*dsql`([\s\S]*?)`\s*;?/g;
const DSQL_CALL_PATTERN =
  /(export\s+)?(?:const|let|var)\s+([A-Za-z_$][A-Za-z0-9_$]*)\s*=\s*dsql\s*\(\s*`([\s\S]*?)`\s*\)\s*;?/g;

export function dsql(options: DsqlVitePluginOptions = {}): VitePlugin {
  const generatedModule = options.generatedModule ?? DEFAULT_GENERATED_MODULE;

  return {
    name: "dsql",
    transform(code, id) {
      if (!isSupportedFile(id) || !code.includes("dsql")) {
        return null;
      }

      return transformDsqlTags(code, generatedModule);
    },
  };
}

export function transformDsqlTags(
  code: string,
  generatedModule = DEFAULT_GENERATED_MODULE,
): TransformResult {
  const imports: string[] = [];
  const exports: string[] = [];
  let changed = false;

  const replaceDsqlBinding = (
    match: string,
    exportKeyword: string | undefined,
    localName: string,
    body: string,
  ): string => {
    if (body.includes("${")) {
      throw new Error("dsql templates do not support JavaScript interpolation");
    }

    const operationName = operationNameFromDsql(body);
    if (!operationName) {
      return match;
    }
    changed = true;

    imports.push(
      `import { ${operationName}Operation as ${localName} } from ${JSON.stringify(
        generatedModule,
      )};`,
    );
    if (exportKeyword) {
      exports.push(`export { ${localName} };`);
    }

    return "";
  };

  const transformed = code
    .replace(DSQL_CALL_PATTERN, replaceDsqlBinding)
    .replace(DSQL_TAG_PATTERN, replaceDsqlBinding);

  if (!changed) {
    return null;
  }

  return {
    code: [...imports, transformed.trimStart(), ...exports]
      .filter((part) => part.length > 0)
      .join("\n"),
    map: null,
  };
}

function isSupportedFile(id: string): boolean {
  const path = id.split("?")[0] ?? id;
  return SUPPORTED_EXTENSIONS.some((extension) => path.endsWith(extension));
}

function operationNameFromDsql(source: string): string | undefined {
  return /\bquery\s+([A-Za-z_][A-Za-z0-9_]*)\b/.exec(source)?.[1];
}

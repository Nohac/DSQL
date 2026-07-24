import { posix, relative, resolve } from "node:path";
import type { DsqlProjectContractFingerprint } from "./daemon.ts";
import {
  projectRelative,
  renderDsql,
  type BuildArtifacts,
  type DsqlDesiredFile,
  type DsqlRenderer,
  type DsqlRendererContext,
  type DsqlRenderModule,
  type DsqlRenderResult,
} from "./node.ts";

export type DsqlProjectScope = {
  readonly imports: readonly string[];
};

export type DsqlProjectContract<
  Scopes extends Record<string, DsqlProjectScope>,
  Targets extends readonly (keyof Scopes & string)[],
  Directives extends Record<string, unknown>,
> = {
  readonly contractHash: DsqlProjectContractFingerprint;
  readonly scopes: Scopes;
  readonly targets: Targets;
  readonly directives: Directives;
};

export type DsqlTargetOutput = {
  readonly ownedRoots: readonly string[];
  readonly targetDirectory: (target: string) => string;
};

export type DsqlFileCollector = {
  /** Writes a UTF-8 file relative to this target's output directory. */
  write(path: string, contents: string): void;
};

export type DsqlDefinitionOutput = {
  /** The checked definition modules emitted earlier in this target's pipeline. */
  readonly current: DsqlRenderResult;
};

export type DsqlProjectGeneratorContext<Target extends string = string> = {
  readonly target: Target;
  readonly projectBase: string;
  readonly outputDirectory: string;
  readonly artifacts: BuildArtifacts;
  readonly embeddedSources: ReadonlyMap<string, string>;
  readonly files: DsqlFileCollector;
  readonly definitions: DsqlDefinitionOutput;
  readonly mode: string;
  readonly command: "serve" | "build";
};

export type DsqlProjectGenerator<Target extends string = string> = {
  readonly name: string;
  readonly targets?: readonly string[];
  /**
   * Type-only contravariant marker: a generator accepting every project target
   * can be wired to one target, while a restricted generator cannot.
   */
  readonly __acceptsTarget?: (target: Target) => void;
  render(context: DsqlProjectGeneratorContext<Target>): void | Promise<void>;
};

type TargetDecision<Target extends string> =
  | {
      readonly generators: readonly DsqlProjectGenerator<Target>[];
    }
  | DsqlIgnoredTarget;

type DsqlIgnoredTarget = {
  readonly kind: "ignore";
};

type ProjectRendererConfig<Target extends string> = {
  readonly output: DsqlTargetOutput;
  readonly targets: { readonly [Name in Target]: TargetDecision<Name> };
};

export type DsqlProject<
  Scopes extends Record<string, DsqlProjectScope>,
  Targets extends readonly (keyof Scopes & string)[],
  Directives extends Record<string, unknown>,
> = {
  readonly contract: DsqlProjectContract<Scopes, Targets, Directives>;
  generator<
    const Allowed extends readonly Targets[number][] | undefined = undefined,
  >(
    generator: Omit<
      DsqlProjectGenerator<
        Allowed extends readonly Targets[number][]
          ? Allowed[number]
          : Targets[number]
      >,
      "targets"
    > &
      (Allowed extends readonly Targets[number][]
        ? { readonly targets: Allowed }
        : { readonly targets?: never }),
  ): DsqlProjectGenerator<
    Allowed extends readonly Targets[number][] ? Allowed[number] : Targets[number]
  >;
  ignore(): DsqlIgnoredTarget;
  renderer(config: ProjectRendererConfig<Targets[number]>): DsqlRenderer;
};

/** Defines the generated, literal-typed project renderer contract. */
export function defineDsqlProject<
  const Scopes extends Record<string, DsqlProjectScope>,
  const Targets extends readonly (keyof Scopes & string)[],
  const Directives extends Record<string, unknown>,
>(
  contract: DsqlProjectContract<Scopes, Targets, Directives>,
): DsqlProject<Scopes, Targets, Directives> {
  const targetSet = new Set<string>();
  for (const target of contract.targets) {
    if (!Object.hasOwn(contract.scopes, target)) {
      throw new Error(`dsql project target ${target} is not a configured scope`);
    }
    if (targetSet.has(target)) {
      throw new Error(`dsql project target ${target} is declared more than once`);
    }
    targetSet.add(target);
  }

  const project: DsqlProject<Scopes, Targets, Directives> = {
    contract,
    generator(generator) {
      return generator;
    },
    ignore() {
      return { kind: "ignore" };
    },
    renderer(config: ProjectRendererConfig<Targets[number]>) {
      return createProjectRenderer(contract, config);
    },
  };
  return project;
}

/** Creates the default target-qualified output layout. */
export function targetOutput(root: string): DsqlTargetOutput {
  const normalized = normalizeRelativePath(root, "target output root");
  return {
    ownedRoots: [normalized],
    targetDirectory(target) {
      return posix.join(normalized, encodeURIComponent(target));
    },
  };
}

export type TypeScriptDefinitionsOptions = {
  readonly queriesDir?: string;
  readonly executionDir?: string;
};

/**
 * Framework-neutral operation and fragment definitions. This generator
 * publishes the module mappings required by embedded callsite rewriting.
 */
export function typescriptDefinitions(
  options: TypeScriptDefinitionsOptions = {},
): Omit<DsqlProjectGenerator<string>, "targets"> {
  return {
    name: "typescript-definitions",
    async render(context) {
      const targetRoot = resolve(context.projectBase, context.outputDirectory);
      const queriesDir = resolve(
        targetRoot,
        normalizeRelativePath(options.queriesDir ?? "queries", "queries directory"),
      );
      const executionDir = options.executionDir
        ? resolve(
            targetRoot,
            normalizeRelativePath(options.executionDir, "execution directory"),
          )
        : undefined;
      const rendered = await renderDsql(context.artifacts, {
        root: context.projectBase,
        scope: {
          name: context.target,
          imports: context.artifacts.scopes[0]?.imports ?? [],
        },
        queriesDir,
        ...(executionDir ? { executionDir } : {}),
        embeddedSources: context.embeddedSources,
      });
      for (const file of rendered.files) {
        const path = normalizeRelativePath(
          relative(targetRoot, file.path).split("\\").join("/"),
          "definition file",
        );
        context.files.write(path, file.contents);
      }
      publishDefinitions(context.definitions, rendered);
    },
  };
}

type MutableDefinitionOutput = DsqlDefinitionOutput & {
  readonly published: boolean;
  publish(result: DsqlRenderResult): void;
};

function createProjectRenderer<
  Scopes extends Record<string, DsqlProjectScope>,
  Targets extends readonly (keyof Scopes & string)[],
  Directives extends Record<string, unknown>,
>(
  contract: DsqlProjectContract<Scopes, Targets, Directives>,
  config: ProjectRendererConfig<Targets[number]>,
): DsqlRenderer {
  validateRendererTargets(contract.targets, config.targets);
  return {
    projectContractHash: contract.contractHash,
    ownedRoots: config.output.ownedRoots,
    async render(context) {
      const groups = validateCompilerScopes(
        contract,
        context.artifacts.artifactGroups,
      );
      const targets = new Map(
        [...groups].filter(([, artifacts]) => artifacts.scopes[0]?.generationTarget),
      );

      const files = new Map<string, string>();
      const modules = new Map<string, DsqlRenderModule>();
      const callsiteTargets = new Set(
        context.result.callsites.flatMap((callsite) =>
          callsite.expressions.map((expression) => expression.target),
        ),
      );
      for (const target of contract.targets) {
        const decision = config.targets[target];
        const artifacts = compilerScope(targets, target);
        if ("kind" in decision) {
          if (targetOwnsCallsite(context, target)) {
            throw new Error(
              `dsql target ${target} owns embedded callsites and cannot be ignored`,
            );
          }
          continue;
        }
        const outputDirectory = config.output.targetDirectory(target);
        const definitions = definitionOutput(modules, callsiteTargets);
        const collector = fileCollector(outputDirectory, files);
        for (const generator of decision.generators) {
          try {
            await generator.render({
              target,
              projectBase: context.projectBase,
              outputDirectory,
              artifacts,
              embeddedSources: context.embeddedSources,
              files: collector,
              definitions,
              mode: context.mode,
              command: context.command,
            });
          } catch (error) {
            throw new Error(
              `dsql generator ${generator.name} failed for target ${target}`,
              { cause: error },
            );
          }
        }
        if (targetOwnsCallsite(context, target) && !definitions.published) {
          throw new Error(
            `dsql target ${target} owns embedded callsites but emitted no TypeScript definitions`,
          );
        }
      }

      return {
        modules: [...modules.values()].sort((left, right) =>
          left.id.localeCompare(right.id),
        ),
        ownedRoots: config.output.ownedRoots,
        files: [...files]
          .sort(([left], [right]) => left.localeCompare(right))
          .map(([path, contents]) => ({ path, contents })),
      };
    },
  };
}

function compilerScope(
  scopes: ReadonlyMap<string, BuildArtifacts>,
  name: string,
): BuildArtifacts {
  const scope = scopes.get(name);
  if (!scope) {
    throw new Error(
      `dsql compiler omitted validated generation target ${name}`,
    );
  }
  return scope;
}

function validateCompilerScopes<
  Scopes extends Record<string, DsqlProjectScope>,
  Targets extends readonly (keyof Scopes & string)[],
  Directives extends Record<string, unknown>,
>(
  contract: DsqlProjectContract<Scopes, Targets, Directives>,
  artifactGroups: readonly BuildArtifacts[],
): Map<string, BuildArtifacts> {
  const groups = new Map<string, BuildArtifacts>();
  for (const artifacts of artifactGroups) {
    const scope = artifacts.scopes[0];
    if (!scope) {
      throw new Error("dsql compiler returned an artifact group without a scope");
    }
    if (groups.has(scope.name)) {
      throw new Error(`dsql compiler returned scope ${scope.name} more than once`);
    }
    groups.set(scope.name, artifacts);
  }
  const expected = Object.keys(contract.scopes);
  const missing = expected.filter((scope) => !groups.has(scope));
  const unexpected = [...groups.keys()].filter(
    (scope) => !Object.hasOwn(contract.scopes, scope),
  );
  if (missing.length > 0 || unexpected.length > 0) {
    throw new Error(
      "dsql compiler scope graph disagrees with the generated project contract" +
        `${missing.length > 0 ? `; missing ${missing.join(", ")}` : ""}` +
        `${unexpected.length > 0 ? `; unexpected ${unexpected.join(", ")}` : ""}`,
    );
  }
  const targetSet = new Set<string>(contract.targets);
  for (const name of expected) {
    const actual = groups.get(name)?.scopes[0];
    const expectedImports = [...(contract.scopes[name]?.imports ?? [])].sort();
    const actualImports = [...(actual?.imports ?? [])].sort();
    if (
      !actual ||
      actual.generationTarget !== targetSet.has(name) ||
      expectedImports.length !== actualImports.length ||
      expectedImports.some((scope, index) => scope !== actualImports[index])
    ) {
      throw new Error(
        `dsql compiler scope ${name} disagrees with the generated project contract`,
      );
    }
  }
  return groups;
}

function validateRendererTargets(
  targets: readonly string[],
  decisions: Readonly<Record<string, unknown>>,
): void {
  const expected = new Set(targets);
  const configured = Object.keys(decisions);
  const missing = targets.filter((target) => !Object.hasOwn(decisions, target));
  const unknown = configured.filter((target) => !expected.has(target));
  if (missing.length > 0 || unknown.length > 0) {
    throw new Error(
      "dsql renderer targets disagree with the generated project contract" +
        `${missing.length > 0 ? `; missing ${missing.join(", ")}` : ""}` +
        `${unknown.length > 0 ? `; unknown ${unknown.join(", ")}` : ""}`,
    );
  }
  for (const target of targets) {
    const decision = decisions[target] as TargetDecision<string> | undefined;
    if (!decision || "kind" in decision) {
      continue;
    }
    for (const generator of decision.generators) {
      if (generator.targets && !generator.targets.includes(target)) {
        throw new Error(
          `dsql generator ${generator.name} cannot run for target ${target}`,
        );
      }
    }
  }
}

function fileCollector(
  outputDirectory: string,
  desired: Map<string, string>,
): DsqlFileCollector {
  return {
    write(path, contents) {
      const relativePath = normalizeRelativePath(path, "generated file");
      const targetPath = posix.join(outputDirectory, relativePath);
      if (desired.has(targetPath)) {
        throw new Error(`dsql generators both write ${targetPath}`);
      }
      desired.set(targetPath, contents);
    },
  };
}

function definitionOutput(
  modules: Map<string, DsqlRenderModule>,
  callsiteTargets: ReadonlySet<string>,
): MutableDefinitionOutput {
  let current: DsqlRenderResult | undefined;
  return {
    get published() {
      return current !== undefined;
    },
    get current() {
      if (!current) {
        throw new Error(
          "dsql generator requires TypeScript definitions; place typescriptDefinitions() earlier",
        );
      }
      return current;
    },
    publish(result) {
      if (current) {
        throw new Error("dsql target emitted TypeScript definitions more than once");
      }
      current = result;
      for (const definition of Object.values(result.definitions)) {
        if (!definition.id) {
          throw new Error(`dsql definition ${definition.name} has no artifact id`);
        }
        if (!callsiteTargets.has(definition.id)) {
          continue;
        }
        if (modules.has(definition.id)) {
          throw new Error(`dsql artifact ${definition.id} is mapped more than once`);
        }
        modules.set(definition.id, {
          id: definition.id,
          module: definition.modulePath,
          export: definition.exportName,
        });
      }
    },
  };
}

function publishDefinitions(
  output: DsqlDefinitionOutput,
  result: DsqlRenderResult,
): void {
  (output as MutableDefinitionOutput).publish(result);
}

function targetOwnsCallsite(context: DsqlRendererContext, target: string): boolean {
  const prefix = `${target}/`;
  return context.result.callsites.some((callsite) =>
    callsite.expressions.some((expression) => expression.target.startsWith(prefix)),
  );
}

function normalizeRelativePath(path: string, role: string): string {
  if (
    path.length === 0 ||
    path.startsWith("/") ||
    /^[A-Za-z]:[/\\]/.test(path) ||
    path.includes("\\")
  ) {
    throw new Error(`dsql ${role} must be a project-relative POSIX path: ${path}`);
  }
  const normalized = posix.normalize(path);
  if (
    normalized === "." ||
    normalized === ".." ||
    normalized.startsWith("../")
  ) {
    throw new Error(`dsql ${role} escapes its output root: ${path}`);
  }
  return normalized;
}

/** Converts absolute or project-relative generated paths for custom generators. */
export function generatedFile(
  projectBase: string,
  path: string,
  contents: string,
): DsqlDesiredFile {
  return {
    path: projectRelative(projectBase, resolve(projectBase, path)),
    contents,
  };
}

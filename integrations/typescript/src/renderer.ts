export { Project, QuoteKind, VariableDeclarationKind } from "ts-morph";
export {
  defineDsqlProject,
  generatedFile,
  targetOutput,
  typescriptDefinitions,
} from "./project.ts";
export type {
  DsqlDefinitionOutput,
  DsqlFileCollector,
  DsqlProject,
  DsqlProjectContract,
  DsqlProjectGenerator,
  DsqlProjectGeneratorContext,
  DsqlProjectScope,
  DsqlTargetOutput,
  TypeScriptDefinitionsOptions,
} from "./project.ts";

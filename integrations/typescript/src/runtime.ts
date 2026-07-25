import type {
  DynamicInputMetadata,
  DynamicInputSite,
  DynamicInputSiteField,
} from "./generated/metadata.ts";

export type DsqlOperation<
  Result = unknown,
  Params = Record<string, never>,
  Input = Record<string, never>,
  Context = Record<string, never>,
> = {
  readonly id: string;
  readonly name: string;
  readonly kind: "query";
  /** Whether server execution binds at least one trusted-context value. */
  readonly requiresContext: boolean;
  /** Public input metadata used for defaults and stable client cache keys. */
  readonly inputs: readonly DsqlInputField[];
  readonly result?: Result;
  readonly params?: Params;
  readonly input?: Input;
  readonly context?: Context;
};

export type DsqlOperationResult<Operation> =
  Operation extends DsqlOperation<infer Result, any, any, any> ? Result : never;

export type DsqlOperationParams<Operation> =
  Operation extends DsqlOperation<any, infer Params, any, any>
    ? Params
    : Record<string, never>;

export type DsqlOperationInput<Operation> =
  Operation extends DsqlOperation<any, any, infer Input, any>
    ? Input
    : Record<string, never>;

export type DsqlOperationContext<Operation> =
  Operation extends DsqlOperation<any, any, any, infer Context>
    ? Context
    : Record<string, never>;

export type DsqlVariables<
  Operation extends DsqlOperation<any, any, any, any>,
> = ({} extends DsqlOperationParams<Operation>
  ? { readonly params?: DsqlOperationParams<Operation> }
  : { readonly params: DsqlOperationParams<Operation> }) &
  ({} extends DsqlOperationInput<Operation>
    ? { readonly input?: DsqlOperationInput<Operation> }
    : { readonly input: DsqlOperationInput<Operation> });

export type DsqlQueryKey<Variables> = readonly [
  "dsql",
  string,
  string | null,
  Variables,
];

export function dsqlQueryKey<Variables>(
  operation: DsqlOperation<any, any, any, any>,
  variables: Variables,
  contextScope?: string,
): DsqlQueryKey<Variables> {
  if (operation.requiresContext && contextScope === undefined) {
    throw new Error(
      `dsql operation ${operation.name} requires contextScope for cache identity`,
    );
  }
  const materialized = canonicalDsqlCacheValue(
    materializeDsqlBindings(operation.inputs, variables),
  );
  return [
    "dsql",
    operation.name,
    contextScope ?? null,
    materialized as Variables,
  ] as const;
}

function canonicalDsqlCacheValue(value: unknown): unknown {
  if (Array.isArray(value)) {
    return value.map(canonicalDsqlCacheValue);
  }
  if (!isDsqlEnvelope(value)) {
    return value;
  }
  return Object.fromEntries(
    Object.keys(value)
      .sort()
      .map((key) => [key, canonicalDsqlCacheValue(value[key])]),
  );
}

export type DsqlFragmentDefinition<
  Result = unknown,
  Params = Record<string, never>,
  Input = Record<string, never>,
> = {
  readonly name: string;
  readonly kind: "fragment";
  readonly table: string;
  readonly result?: Result;
  readonly params?: Params;
  readonly input?: Input;
};

export type DsqlFragment<Fragment> =
  Fragment extends DsqlFragmentDefinition<infer Result, any, any>
    ? Result
    : never;

export type DsqlFragmentParams<Fragment> =
  Fragment extends DsqlFragmentDefinition<any, infer Params, any>
    ? Params
    : Record<string, never>;

export type DsqlFragmentInput<Fragment> =
  Fragment extends DsqlFragmentDefinition<any, any, infer Input>
    ? Input
    : Record<string, never>;

export type DsqlFragmentVariables<Fragment> =
  ({} extends DsqlFragmentParams<Fragment>
    ? { readonly params?: DsqlFragmentParams<Fragment> }
    : { readonly params: DsqlFragmentParams<Fragment> }) &
  ({} extends DsqlFragmentInput<Fragment>
    ? { readonly input?: DsqlFragmentInput<Fragment> }
    : { readonly input: DsqlFragmentInput<Fragment> });

export type DsqlDefinition = DsqlOperation | DsqlFragmentDefinition;

export interface DsqlSourceRegistry {}

export type DsqlDefinitionForSource<Source extends string> =
  Source extends keyof DsqlSourceRegistry
    ? DsqlSourceRegistry[Source]
    : DsqlDefinition;

export type DsqlSqlParameter = {
  readonly path: string;
};

export type DsqlSqlVariants = Record<
  string,
  {
    readonly cases: Record<string, string>;
    readonly nullText?: string;
  }
>;

export type DsqlInputDefault = {
  readonly kind: string;
  readonly value?: string;
  readonly boolean?: boolean;
  readonly items?: readonly DsqlInputDefault[];
};

export type DsqlInputField = {
  readonly path: string;
  readonly data_type: string;
  readonly collection?: boolean;
  readonly enum_values?: readonly string[];
  readonly required: boolean;
  readonly nullable: boolean;
  readonly default?: DsqlInputDefault;
};

export type DsqlExecutionPayload<
  Operation extends DsqlOperation<any, any, any, any> = DsqlOperation<any, any, any, any>,
> = {
  readonly operation: Operation;
  readonly sql: string;
  readonly parameters: readonly DsqlSqlParameter[];
  readonly variants: DsqlSqlVariants;
  readonly dynamicInputs: readonly DynamicInputMetadata[];
  readonly inputs: readonly DsqlInputField[];
};

export type DsqlMaterializedQuery = {
  readonly sql: string;
  readonly values: readonly unknown[];
};

export function dsql<const Source extends string>(
  source: Source,
): DsqlDefinitionForSource<Source>;
export function dsql(
  strings: TemplateStringsArray,
  ...values: never[]
): DsqlDefinition;
export function dsql(): DsqlDefinition {
  // Callsite detection is Rust-extractor-owned and rewriting is the
  // build binding's job (docs/spec/build-daemon.md, Callsites): an
  // expression reaching this function at runtime means no binding
  // transformed the module, and evaluating the untyped fallback would
  // silently ship a broken operation.
  throw new Error(
    "dsql callsite was not transformed - is the dsql build plugin installed " +
      "and running before other transforms?",
  );
}

export function materializeDsqlQuery<
  Operation extends DsqlOperation<any, any, any, any>,
>(
  payload: DsqlExecutionPayload<Operation>,
  variables: DsqlVariables<Operation>,
  context: DsqlOperationContext<Operation>,
): DsqlMaterializedQuery {
  const bindings = materializeDsqlBindings(payload.inputs, {
    ...variables,
    context,
  });
  const fixedValues = collectDsqlParameterValues(payload.parameters, bindings);
  const dynamic = applyDsqlDynamicInputs(
    applyDsqlVariants(payload.sql, payload.variants, bindings),
    payload.dynamicInputs,
    bindings,
    fixedValues.length,
  );
  return {
    sql: dynamic.sql,
    values: [...fixedValues, ...dynamic.values],
  };
}

export function applyDsqlVariants(
  sql: string,
  variants: DsqlSqlVariants,
  variables: unknown,
): string {
  let rendered = sql;
  for (const [path, variant] of Object.entries(variants)) {
    const selected = getDsqlPath(variables, path);
    const nullText = variant.nullText;
    if (selected == null && nullText !== undefined) {
      rendered = rendered.replaceAll(`{{${path}}}`, () => nullText);
      continue;
    }
    if (typeof selected !== "string") {
      throw new Error(`missing dsql variant value at ${path}`);
    }

    const replacement = variant.cases[selected];
    if (replacement === undefined) {
      throw new Error(`invalid dsql variant value at ${path}: ${selected}`);
    }
    rendered = rendered.replaceAll(`{{${path}}}`, () => replacement);
  }
  return rendered;
}

function applyDsqlDynamicInputs(
  sql: string,
  inputs: readonly DynamicInputMetadata[],
  bindings: unknown,
  fixedParameterCount: number,
): { readonly sql: string; readonly values: readonly unknown[] } {
  let renderedSql = sql;
  const values: unknown[] = [];
  for (const input of inputs) {
    let value = getDsqlPath(bindings, input.path);
    if (value === null) {
      value = input.kind === "predicate" ? {} : [];
    }
    if (value === undefined) {
      throw new Error(`missing dsql dynamic input at ${input.path}`);
    }
    const inputStart = values.length;
    let expectedEnd: number | undefined;
    if (input.sites.length === 0) {
      throw new Error(`dsql dynamic input ${input.path} has no SQL sites`);
    }
    for (const site of input.sites) {
      const state = {
        values,
        cursor: inputStart,
        fixedParameterCount,
      };
      const replacement =
        input.kind === "predicate"
          ? renderDsqlDynamicPredicate(input, site, value, input.path, state) ??
            site.identity_sql
          : input.kind === "order"
            ? renderDsqlDynamicOrder(input, site, value, input.path) ??
              site.identity_sql
            : invalidDsqlDynamicMetadata(
                `input ${input.path} has unknown kind ${input.kind}`,
              );
      if (expectedEnd !== undefined && state.cursor !== expectedEnd) {
        invalidDsqlDynamicMetadata(
          `sites for ${input.path} consume different operands`,
        );
      }
      expectedEnd ??= state.cursor;
      const markerIndex = renderedSql.indexOf(site.marker);
      if (
        markerIndex < 0 ||
        renderedSql.indexOf(site.marker, markerIndex + site.marker.length) >= 0
      ) {
        invalidDsqlDynamicMetadata(
          `marker ${site.marker} does not identify exactly one SQL site`,
        );
      }
      renderedSql =
        renderedSql.slice(0, markerIndex) +
        replacement +
        renderedSql.slice(markerIndex + site.marker.length);
    }
  }
  return { sql: renderedSql, values };
}

type DynamicRenderState = {
  readonly values: unknown[];
  cursor: number;
  readonly fixedParameterCount: number;
};

function renderDsqlDynamicPredicate(
  input: DynamicInputMetadata,
  site: DynamicInputSite,
  value: unknown,
  path: string,
  state: DynamicRenderState,
): string | undefined {
  if (!isDsqlEnvelope(value)) {
    throw new Error(`dsql input ${path} must be a dynamic predicate object`);
  }
  const predicates: string[] = [];
  for (const key of Object.keys(value).sort()) {
    const item = value[key];
    if (key === "and" || key === "or") {
      if (!Array.isArray(item)) {
        throw new Error(`dsql input ${path}.${key} must be an array`);
      }
      const children = item.flatMap((child, index) => {
        const rendered = renderDsqlDynamicPredicate(
          input,
          site,
          child,
          `${path}.${key}[${index}]`,
          state,
        );
        return rendered === undefined ? [] : [rendered];
      });
      if (key === "or") {
        predicates.push(
          children.length === 0
            ? "FALSE"
            : parenthesizedDsqlJoin(children, " OR "),
        );
      } else if (children.length > 0) {
        predicates.push(parenthesizedDsqlJoin(children, " AND "));
      }
      continue;
    }
    if (key === "not") {
      const child = renderDsqlDynamicPredicate(
        input,
        site,
        item,
        `${path}.not`,
        state,
      );
      if (child !== undefined) {
        predicates.push(`NOT (${child})`);
      }
      continue;
    }

    const field = dsqlDynamicSiteField(input, site, key, path);
    if (!isDsqlEnvelope(item)) {
      throw new Error(
        `dsql input ${path}.${key} must be a field-operator object`,
      );
    }
    const dataType = dsqlDynamicFieldDataType(input, key);
    for (const operatorName of Object.keys(item).sort()) {
      const operand = item[operatorName];
      const operator = field.operators.find(
        (candidate) => candidate.name === operatorName,
      );
      if (operator === undefined) {
        throw new Error(
          `invalid dsql dynamic operator at ${path}.${key}.${operatorName}`,
        );
      }
      const operandPath = `${path}.${key}.${operatorName}`;
      if (operator.value_kind === "boolean") {
        if (typeof operand !== "boolean") {
          throw new Error(`dsql input ${operandPath} must be a boolean`);
        }
        const selected = operator.cases.find(
          (candidate) => candidate.value === String(operand),
        );
        if (selected === undefined) {
          invalidDsqlDynamicMetadata(
            `operator ${operatorName} has no boolean lowering`,
          );
        }
        predicates.push(selected.text);
        continue;
      }
      if (operator.value_kind === "collection") {
        if (!Array.isArray(operand)) {
          throw new Error(`dsql input ${operandPath} must be an array`);
        }
        for (const value of operand) {
          validateDsqlDynamicScalar(dataType, value, operandPath);
        }
        if (operand.length === 0) {
          predicates.push(operatorName === "in" ? "FALSE" : "TRUE");
          continue;
        }
      } else if (operator.value_kind === "scalar") {
        validateDsqlDynamicScalar(dataType, operand, operandPath);
      } else {
        invalidDsqlDynamicMetadata(
          `operator ${operatorName} has unknown value kind ${operator.value_kind}`,
        );
      }
      const placeholder = dsqlDynamicParameter(state, operand);
      predicates.push(
        `${operator.before_value ?? ""}${placeholder}${operator.after_value ?? ""}`,
      );
    }
  }
  return predicates.length === 0
    ? undefined
    : parenthesizedDsqlJoin(predicates, " AND ");
}

function renderDsqlDynamicOrder(
  input: DynamicInputMetadata,
  site: DynamicInputSite,
  value: unknown,
  path: string,
): string | undefined {
  if (!Array.isArray(value)) {
    throw new Error(`dsql input ${path} must be an array of order entries`);
  }
  const rendered = value.map((entry, index) => {
    const entryPath = `${path}[${index}]`;
    if (!isDsqlEnvelope(entry) || Object.keys(entry).length !== 1) {
      throw new Error(`dsql input ${entryPath} must contain exactly one field`);
    }
    const [key] = Object.keys(entry);
    if (key === undefined) {
      throw new Error(`dsql input ${entryPath} must contain exactly one field`);
    }
    const field = dsqlDynamicSiteField(input, site, key, path);
    const direction = entry[key];
    if (typeof direction !== "string") {
      throw new Error(`dsql input ${entryPath} must contain an order direction`);
    }
    const selected = field.directions.find(
      (candidate) => candidate.value === direction,
    );
    if (selected === undefined) {
      throw new Error(`invalid dsql order direction at ${entryPath}`);
    }
    return selected.text;
  });
  return rendered.length === 0 ? undefined : rendered.join(", ");
}

function dsqlDynamicSiteField(
  input: DynamicInputMetadata,
  site: DynamicInputSite,
  key: string,
  path: string,
): DynamicInputSiteField {
  if (!input.fields.some((field) => field.key === key)) {
    throw new Error(`unknown dsql dynamic field at ${path}.${key}`);
  }
  const field = site.fields.find((candidate) => candidate.key === key);
  return (
    field ??
    invalidDsqlDynamicMetadata(
      `site ${site.marker} has no lowering for field ${key}`,
    )
  );
}

function dsqlDynamicFieldDataType(
  input: DynamicInputMetadata,
  key: string,
): string {
  return (
    input.fields.find((field) => field.key === key)?.data_type ??
    invalidDsqlDynamicMetadata(`input ${input.path} has no field ${key}`)
  );
}

function dsqlDynamicParameter(
  state: DynamicRenderState,
  value: unknown,
): string {
  const existing = state.values[state.cursor];
  if (state.cursor === state.values.length) {
    state.values.push(value);
  } else if (existing !== value) {
    invalidDsqlDynamicMetadata("reused dynamic site disagrees on an operand");
  }
  state.cursor += 1;
  return `$${state.fixedParameterCount + state.cursor}`;
}

function validateDsqlDynamicScalar(
  dataType: string,
  value: unknown,
  path: string,
): void {
  if (value === null || value === undefined) {
    throw new Error(`dsql input ${path} must be a non-null ${dataType}`);
  }
  const valid =
    dataType === "text"
      ? typeof value === "string"
      : dataType === "uuid"
        ? typeof value === "string" && DSQL_UUID.test(value)
        : dataType === "timestamptz"
          ? typeof value === "string" && isDsqlRfc3339Timestamp(value)
      : dataType === "int"
        ? Number.isSafeInteger(value)
        : dataType === "float"
          ? typeof value === "number" && Number.isFinite(value)
          : dataType === "numeric"
            ? typeof value === "string" && DSQL_NUMERIC.test(value)
            : dataType === "boolean"
              ? typeof value === "boolean"
              : dataType === "json";
  if (!valid) {
    throw new Error(`dsql input ${path} must be a valid ${dataType}`);
  }
}

const DSQL_UUID =
  /^(?:[0-9a-f]{32}|(?:urn:uuid:)?[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}|\{[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\})$/i;
const DSQL_RFC_3339_TIMESTAMP =
  /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})(?:\.\d+)?(?:Z|[+-](\d{2}):(\d{2}))$/;
const DSQL_NUMERIC = /^[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?$/;

function isDsqlRfc3339Timestamp(value: string): boolean {
  const match = DSQL_RFC_3339_TIMESTAMP.exec(value);
  if (match === null) {
    return false;
  }
  const [year, month, day, hour, minute, second, offsetHour, offsetMinute] =
    match.slice(1).map(Number);
  if (
    year === undefined ||
    month === undefined ||
    day === undefined ||
    hour === undefined ||
    minute === undefined ||
    second === undefined ||
    month < 1 ||
    month > 12 ||
    hour > 23 ||
    minute > 59 ||
    second > 59 ||
    (offsetHour ?? 0) > 23 ||
    (offsetMinute ?? 0) > 59
  ) {
    return false;
  }
  const leapYear = year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0);
  const daysInMonth = [
    31,
    leapYear ? 29 : 28,
    31,
    30,
    31,
    30,
    31,
    31,
    30,
    31,
    30,
    31,
  ][month - 1];
  return daysInMonth !== undefined && day >= 1 && day <= daysInMonth;
}

function parenthesizedDsqlJoin(
  values: readonly string[],
  separator: string,
): string {
  return `(${values.join(separator)})`;
}

function invalidDsqlDynamicMetadata(message: string): never {
  throw new Error(`invalid dsql dynamic metadata: ${message}`);
}

export function materializeDsqlBindings(
  fields: readonly DsqlInputField[],
  bindings: unknown,
): unknown {
  let materialized = bindings;
  for (const field of fields) {
    const trustedContext = field.path.startsWith("context.");
    const lookup = lookupDsqlPath(materialized, field.path);
    if (lookup.kind === "invalid") {
      throw new Error(`invalid dsql input envelope at ${field.path}`);
    }

    let value = lookup.kind === "found" ? lookup.value : undefined;
    if (lookup.kind === "missing") {
      if (trustedContext) {
        throw new Error(`missing trusted dsql context at ${field.path}`);
      }
      if (field.default !== undefined) {
        value = dsqlDefaultValue(field, field.default);
        materialized = setDsqlPath(materialized, field.path, value);
      } else if (field.required) {
        throw new Error(`missing dsql input at ${field.path}`);
      }
    }

    if (value === null && !field.nullable) {
      throw new Error(
        trustedContext
          ? `missing trusted dsql context at ${field.path}`
          : `non-null dsql input is null at ${field.path}`,
      );
    }
    if (
      value === null &&
      (field.data_type === "dynamic_predicate" ||
        field.data_type === "dynamic_order")
    ) {
      const identity = field.data_type === "dynamic_predicate" ? {} : [];
      materialized = setDsqlPath(materialized, field.path, identity);
    }
    validateDsqlSafeInteger(field, value);
  }
  return materialized;
}

function validateDsqlSafeInteger(
  field: DsqlInputField,
  value: unknown,
): void {
  if (value === null || value === undefined || field.data_type !== "int") {
    return;
  }
  if (field.collection === true) {
    if (
      !Array.isArray(value) ||
      !value.every((item) => item === null || Number.isSafeInteger(item))
    ) {
      throw new Error(
        `operation input ${field.path} must be an array of safe integers`,
      );
    }
    return;
  }
  if (!Number.isSafeInteger(value)) {
    throw new Error(`operation input ${field.path} must be a safe integer`);
  }
}

function dsqlDefaultValue(
  field: DsqlInputField,
  defaultValue: DsqlInputDefault,
): unknown {
  switch (defaultValue.kind) {
    case "string": {
      if (field.collection === true || defaultValue.value === undefined) {
        return invalidDsqlDefault(field, "a valid string default");
      }
      if (
        field.enum_values !== undefined &&
        field.enum_values.length > 0 &&
        !field.enum_values.includes(defaultValue.value)
      ) {
        return invalidDsqlDefault(field, "a declared string default");
      }
      return defaultValue.value;
    }
    case "number": {
      if (field.collection === true || defaultValue.value === undefined) {
        return invalidDsqlDefault(field, "a valid number default");
      }
      if (field.data_type === "int") {
        if (!/^[+-]?\d+$/.test(defaultValue.value)) {
          return invalidDsqlDefault(field, "an integer default");
        }
        const integer = BigInt(defaultValue.value);
        if (
          integer < BigInt(Number.MIN_SAFE_INTEGER) ||
          integer > BigInt(Number.MAX_SAFE_INTEGER)
        ) {
          return invalidDsqlDefault(field, "a safe integer default");
        }
        return Number(integer);
      }
      if (field.data_type === "float") {
        const value = Number(defaultValue.value);
        if (
          !/^[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?$/.test(
            defaultValue.value,
          ) ||
          !Number.isFinite(value)
        ) {
          return invalidDsqlDefault(field, "a finite float default");
        }
        return value;
      }
      if (field.data_type === "numeric") {
        if (!/^[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?$/.test(defaultValue.value)) {
          return invalidDsqlDefault(field, "a numeric default");
        }
        return defaultValue.value;
      }
      return invalidDsqlDefault(field, "a compatible number default");
    }
    case "boolean": {
      if (
        field.collection === true ||
        field.data_type !== "boolean" ||
        defaultValue.boolean === undefined
      ) {
        return invalidDsqlDefault(field, "a valid boolean default");
      }
      return defaultValue.boolean;
    }
    case "null":
      return null;
    case "collection": {
      if (
        (field.collection !== true && field.data_type !== "dynamic_order") ||
        defaultValue.items === undefined
      ) {
        return invalidDsqlDefault(field, "a valid collection default");
      }
      if (
        field.data_type === "dynamic_order" &&
        defaultValue.items.length !== 0
      ) {
        return invalidDsqlDefault(field, "an empty dynamic order default");
      }
      const itemField = { ...field, collection: false };
      return defaultValue.items.map((item) => {
        if (["null", "collection", "empty_object"].includes(item.kind)) {
          return invalidDsqlDefault(field, "a valid collection default");
        }
        return dsqlDefaultValue(itemField, item);
      });
    }
    case "empty_object":
      if (
        field.collection === true ||
        field.data_type !== "dynamic_predicate"
      ) {
        return invalidDsqlDefault(field, "a dynamic predicate default");
      }
      return {};
    default:
      return invalidDsqlDefault(field, "a recognized input default");
  }
}

function invalidDsqlDefault(field: DsqlInputField, expected: string): never {
  throw new Error(`invalid dsql default at ${field.path}: expected ${expected}`);
}

function setDsqlPath(value: unknown, path: string, replacement: unknown): unknown {
  const parts = path.split(".");
  return setDsqlPathParts(value, parts, 0, replacement, path);
}

function setDsqlPathParts(
  value: unknown,
  parts: readonly string[],
  index: number,
  replacement: unknown,
  path: string,
): unknown {
  if (!isDsqlEnvelope(value)) {
    throw new Error(`cannot materialize dsql input at ${path}`);
  }
  const part = parts[index];
  if (part === undefined) {
    return replacement;
  }
  if (index === parts.length - 1) {
    return { ...value, [part]: replacement };
  }
  const child = value[part] === undefined ? {} : value[part];
  if (!isDsqlEnvelope(child)) {
    throw new Error(`cannot materialize dsql input at ${path}`);
  }
  return {
    ...value,
    [part]: setDsqlPathParts(child, parts, index + 1, replacement, path),
  };
}

export function collectDsqlParameterValues(
  parameters: readonly DsqlSqlParameter[],
  variables: unknown,
): readonly unknown[] {
  return parameters.map((parameter) => getDsqlPath(variables, parameter.path));
}

export function getDsqlPath(value: unknown, path: string): unknown {
  const lookup = lookupDsqlPath(value, path);
  return lookup.kind === "found" ? lookup.value : undefined;
}

type DsqlPathLookup =
  | { readonly kind: "found"; readonly value: unknown }
  | { readonly kind: "missing" }
  | { readonly kind: "invalid" };

function lookupDsqlPath(value: unknown, path: string): DsqlPathLookup {
  if (path === "") {
    return { kind: "found", value };
  }

  let current = value;
  for (const part of path.split(".")) {
    if (!isDsqlEnvelope(current)) {
      return { kind: "invalid" };
    }
    if (!Object.prototype.hasOwnProperty.call(current, part)) {
      return { kind: "missing" };
    }
    current = current[part];
    if (current === undefined) {
      return { kind: "missing" };
    }
  }
  return { kind: "found", value: current };
}

function isDsqlEnvelope(value: unknown): value is Record<string, unknown> {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    return false;
  }
  const prototype = Object.getPrototypeOf(value);
  return prototype === Object.prototype || prototype === null;
}

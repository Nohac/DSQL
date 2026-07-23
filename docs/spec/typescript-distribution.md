# TypeScript Distribution And Project Wiring

Status: design; the current TypeScript integration is not yet packaged this
way.

Adding DSQL to an existing TypeScript/Vite project should require no Rust
toolchain, global executable, copied binary, or hand-written build lifecycle.
The npm packages hide distribution mechanics while keeping framework-specific
generation code visible and editable in the application repository.

This document owns npm package boundaries, native binary selection, onboarding,
and project-owned generator scaffolding. The compiler-to-renderer metadata and
file publication contracts remain defined by [Code Generation
Metadata](codegen.md) and the [build daemon](build-daemon.md).

## Package Boundaries

The initial public packages are:

```text
@dsql/cli
@dsql/typescript
@dsql/vite
@dsql/create
```

- `@dsql/cli` exposes the `dsql` executable shim, selects the packaged native
  executable, and supports explicit custom-binary overrides.
- `@dsql/typescript` contains the browser-safe runtime, metadata types, project
  descriptor and renderer APIs, plus Node-only daemon-client, rendering,
  publication, and command-runner helpers behind explicit export paths.
- `@dsql/vite` contains the Vite binding. It owns Vite lifecycle and watcher
  integration plus callsite rewriting, delegating daemon lifecycle, retry,
  rendering, validation, and publication to `@dsql/typescript`.
- `@dsql/create` is the explicit initializer and scaffold runner. It may edit a
  project only after the user invokes it; installation itself is inert.

The separation is semantic, not an onboarding tax. Generated application code
may import `@dsql/typescript` at runtime, while `@dsql/cli` and `@dsql/vite` are
development/build dependencies. The initializer installs each as a direct
dependency in the correct section so strict package managers do not rely on
transitive dependency visibility.

Backend-only projects and CI use the packaged non-Vite runner:

```text
dsql-typescript generate --config dsql/generate.ts
```

The runner performs the same grouped daemon handoff and renderer orchestration
as the Vite binding without starting Vite. It locates the daemon through the
resolver order below, so `@dsql/cli` remains an explicit development dependency
rather than a hidden transitive requirement. The initializer may add this
command to project package scripts.

Vite is the only maintained host binding initially. The daemon and renderer
contracts remain host-neutral, but this design does not promise unfinished
Webpack, Babel, SWC, or framework-specific bindings.

## Native Packages

`@dsql/cli` declares exact-version optional dependencies on platform payloads:

```text
@dsql/native-darwin-arm64
@dsql/native-darwin-x64
@dsql/native-linux-arm64-gnu
@dsql/native-linux-arm64-musl
@dsql/native-linux-x64-gnu
@dsql/native-linux-x64-musl
@dsql/native-win32-arm64
@dsql/native-win32-x64
```

Each payload uses npm `os`, `cpu`, and, on Linux, `libc` metadata. It contains
the native executable with the platform-correct filename and executable mode.
Package versions are exact and identical across the CLI, binding, TypeScript,
initializer, and native payloads in one release.

The resolver order is:

1. an explicit integration `daemon.command`;
2. `DSQL_BIN`, for repository development and intentional custom builds; and
3. the compatible packaged native payload.

Failure to find the payload is actionable and reports the detected OS,
architecture, and libc where applicable. It must not silently execute an
unrelated `dsql` from `PATH`.

Installation never:

- downloads an executable from an arbitrary URL in `postinstall`;
- compiles Rust;
- modifies the consumer project;
- requires lifecycle scripts to be enabled; or
- installs every platform executable into one package.

Standalone signed archives and checksums may also be published for non-npm
users, editors, and CI, but they are a secondary channel for TypeScript
projects.

## Onboarding

The intended entry point is:

```text
npm create @dsql
```

Equivalent invocations through pnpm, Yarn, and Bun should be documented and
tested. The initializer targets the current directory by default and is safe to
rerun.

For a detected TypeScript/Vite project it:

1. installs `@dsql/typescript` as a runtime dependency and `@dsql/cli` plus
   `@dsql/vite` as development dependencies;
2. creates or updates `dsql/dsql.toml` with explicit document resolvers and
   resolution scopes;
3. uses an environment variable such as `DATABASE_URL` rather than copying a
   credential into tracked configuration;
4. creates the generated project descriptor and project-owned generation
   entrypoint described below;
5. adds the DSQL Vite plugin before source-altering plugins;
6. optionally scaffolds selected framework generators;
7. introspects when the database is reachable; and
8. validates the resulting project.

Every edit is previewable, idempotent, and preserves unrelated configuration.
Existing ambiguous Vite or TypeScript configuration produces instructions
rather than a guessed rewrite. Non-interactive flags may accept defaults, skip
database access, or fail instead of prompting.

Package installation alone performs none of these steps.

## Generated Project Contract

DSQL generates a small TypeScript project descriptor at:

```text
dsql/project.generated.ts
```

It records the exact resolution-scope graph, terminal generation targets and
their effective closures, plus the checked directive type registry needed by
generator APIs. Its contract is specified in [Code Generation
Metadata](codegen.md).

This file is reproducible generated source, not mutable compiler cache or
publication state. It contains no credentials, artifact lists, SQL, or
generation results. Projects may commit it so `generate.ts` type-checks in a
fresh checkout; all supported commands must also be able to recreate it from
project configuration and registered directive schemas. Disposable manifests,
locks, and render state remain under `dsql/build/`.

## Project-Owned Generation Source

The initializer creates ordinary authored files:

```text
dsql/
  generate.ts
  generators/
  templates/
```

`generate.ts` is declarative wiring: it maps typed terminal targets to
project-owned generators. It does not implement daemon lifecycle, artifact
grouping, scope resolution, path ownership, collision detection, stale cleanup,
or DSQL error formatting.

Framework recipes copy generator and template source into the application:

```text
dsql/generators/tanstack-query.ts
dsql/generators/tanstack-start.ts
dsql/templates/tanstack-query.ts
dsql/templates/tanstack-start.ts
```

The copied files become project-owned. They may import stable APIs from
`@dsql/typescript`, but framework-specific behavior does not remain hidden in an
opaque DSQL runtime package. TanStack dependencies remain application
dependencies rather than dependencies of the DSQL runtime.

Initial scaffolding is deliberately one-way:

- existing authored files are never overwritten silently;
- forcing replacement requires explicit user intent;
- copied source may be edited freely; and
- upgrades use release notes or explicit future codemods, not an implicit
  template merge system.

Machine-owned renderer output is written only beneath declared owned roots,
such as:

```text
src/generated/dsql/
```

Authored generators and templates never live beneath those roots.

## Release Gates

A release publishes native payloads before the public packages that reference
them. The public packages are published only after installing packed tarballs
has succeeded on the supported platform matrix.

The release matrix covers:

- macOS arm64 and x64;
- Windows arm64 and x64;
- Linux arm64 and x64 on glibc and musl;
- native binary and daemon startup;
- a real DSQL compilation;
- Node ESM and Bun package loading;
- a minimal Vite development transform and production build;
- npm, pnpm, Yarn, and Bun installation where supported; and
- project paths containing spaces and non-ASCII characters.

Protocol and npm-package versions are separate compatibility checks. The Vite
binding still performs exact daemon protocol negotiation even when npm package
versions match.

## Non-Goals

The initial distribution does not:

- embed the compiler through N-API;
- move compiler semantics into JavaScript;
- require a global installation;
- make the Vite plugin a production runtime dependency;
- hide project-specific framework generation behind a hosted service; or
- promise every Rust compilation target as a supported development platform.

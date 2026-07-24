import { defineDsqlProject } from "@dsql/typescript/renderer";

export const project = defineDsqlProject({
  contractHash: {"algorithm":"sha256","value":"f74951716820a90812635f103c65022b1fbd35aeb3444643e354f01055a3db1a"},
  scopes: {
    ["analytics"]: { imports: ["shared"] },
    ["api"]: { imports: ["shared"] },
    ["shared"]: { imports: [] }
  },
  targets: ["analytics","api"],
  directives: {},
});

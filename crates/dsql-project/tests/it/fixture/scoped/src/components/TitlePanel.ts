import { dsql } from "../dsql";

export const titlePanelQuery = dsql`
query TitlePanel {
  title(limit 1) {
    ...TitleBits
    kind_type {
      kind
    }
  }
}
`;
